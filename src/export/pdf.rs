use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result};
use printpdf::image_crate::{
    self, imageops::FilterType, DynamicImage, GenericImageView, Rgb as Pixel, RgbImage,
};
use printpdf::path::{PaintMode, WindingOrder};
use printpdf::xobject::{ImageFilter, ImageXObject};
use printpdf::{
    calculate_points_for_circle, Color, ColorBits, ColorSpace, Image, ImageTransform,
    IndirectFontRef, Mm, PdfDocument, PdfDocumentReference, PdfLayerReference, PdfPageIndex,
    Polygon, Px, Rect, Rgb,
};

use super::fetch::{capitalize, human_bytes, human_duration};
use super::html::local_time;
use super::model::{ChatExport, MessageExport};
use super::rules::MediaKind;

const FONT_REGULAR: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans-Bold.ttf");

const PAGE_W: f32 = 210.0;
const PAGE_H: f32 = 297.0;
const MARGIN: f32 = 16.0;
const TOP: f32 = PAGE_H - 20.0;
const BOTTOM: f32 = 18.0;
const BODY_PT: f32 = 10.0;
const LINE: f32 = 4.6;
const PT_TO_MM: f32 = 0.352_778;
const MAX_IMAGE_W: f32 = 95.0;
const MAX_IMAGE_H: f32 = 85.0;
const MAX_IMAGE_PX: u32 = 1100;
/// Messages column: avatars sit left of it, the time column sits right of it.
const MSG_X: f32 = MARGIN + 11.0;
const MSG_AVATAR: f32 = 8.0;
const TIME_COL: f32 = 18.0;
/// I start a new sender block after this many seconds of silence, like Telegram does.
const GROUP_GAP_SECS: i64 = 10 * 60;
const MAX_PARTICIPANT_ROWS: usize = 120;

pub struct PdfStamp {
    pub account_name: String,
    pub self_id: i64,
    pub requested_at: i64,
    pub completed_at: i64,
}

/// I render one chat to PDF. The work is CPU-bound, so I run on a blocking thread.
pub fn render_chat(chat: &ChatExport, root: &Path, stamp: &PdfStamp) -> Result<Vec<u8>> {
    let face = ttf_parser::Face::parse(FONT_REGULAR, 0).context("parsing the bundled font")?;
    let bold_face =
        ttf_parser::Face::parse(FONT_BOLD, 0).context("parsing the bundled bold font")?;
    let (doc, page, layer) = PdfDocument::new(
        format!("{} · Telegram export", chat.title),
        Mm(PAGE_W),
        Mm(PAGE_H),
        "content",
    );
    let regular = doc
        .add_external_font_with_subsetting(Cursor::new(FONT_REGULAR), true)
        .context("embedding the regular font")?;
    let bold = doc
        .add_external_font_with_subsetting(Cursor::new(FONT_BOLD), true)
        .context("embedding the bold font")?;
    let mut writer = Writer {
        layer: doc.get_page(page).get_layer(layer),
        doc,
        face,
        bold_face,
        regular,
        bold,
        y: TOP,
        page: 1,
        page_index: page,
        bookmarked: HashSet::new(),
        stamp,
        title: chat.title.clone(),
        root,
        avatars: HashMap::new(),
    };
    writer.cover(chat);
    writer.participants(chat);
    writer.statistics(chat);
    writer.messages(chat);
    writer.footer();
    let Writer { doc, .. } = writer;
    doc.save_to_bytes().context("writing the PDF")
}

type Shade = (f32, f32, f32);
const TEXT: Shade = (0.08, 0.1, 0.13);
const GRAY: Shade = (0.45, 0.49, 0.54);
const RULE: Shade = (0.87, 0.89, 0.92);
const ACCENT: Shade = (0.18, 0.49, 0.88);
const MINE: Shade = (0.18, 0.55, 0.32);
const DANGER: Shade = (0.84, 0.27, 0.36);
const SOFT: Shade = (0.95, 0.96, 0.98);
const WHITE: Shade = (1.0, 1.0, 1.0);

/// Telegram-like sender colors; I pick one from the peer id so a person keeps a color.
const PALETTE: [Shade; 7] = [
    (0.89, 0.33, 0.29),
    (0.93, 0.53, 0.18),
    (0.55, 0.38, 0.86),
    (0.27, 0.65, 0.33),
    (0.16, 0.65, 0.73),
    (0.22, 0.52, 0.89),
    (0.86, 0.33, 0.58),
];

fn palette(seed: i64) -> Shade {
    PALETTE[seed.unsigned_abs() as usize % PALETTE.len()]
}

fn color((r, g, b): Shade) -> Color {
    Color::Rgb(Rgb::new(r, g, b, None))
}

/// I crop a photo to a centered square, scale it and cut a smooth circle on a white page.
fn round_avatar(path: &Path, px: u32) -> Option<DynamicImage> {
    let image = image_crate::open(path).ok()?;
    let (w, h) = image.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let side = w.min(h);
    let square = image
        .crop_imm((w - side) / 2, (h - side) / 2, side, side)
        .resize_exact(px, px, FilterType::Triangle)
        .to_rgba8();
    let radius = px as f32 / 2.0;
    let mut out = RgbImage::new(px, px);
    for (x, y, pixel) in out.enumerate_pixels_mut() {
        let dx = x as f32 + 0.5 - radius;
        let dy = y as f32 + 0.5 - radius;
        let edge = (radius - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
        let source = square.get_pixel(x, y);
        let alpha = edge * f32::from(source[3]) / 255.0;
        let blend = |c: u8| (f32::from(c) * alpha + 255.0 * (1.0 - alpha)).round() as u8;
        *pixel = Pixel([blend(source[0]), blend(source[1]), blend(source[2])]);
    }
    Some(DynamicImage::ImageRgb8(out))
}

/// I embed pictures as JPEG (DCTDecode): a photo stays a few kilobytes instead of raw pixels.
fn jpeg_image(image: &DynamicImage, quality: u8) -> Option<Image> {
    let rgb = image.to_rgb8();
    let (width, height) = rgb.dimensions();
    let mut encoded = Vec::new();
    image_crate::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, quality)
        .encode(rgb.as_raw(), width, height, image_crate::ColorType::Rgb8)
        .ok()?;
    if encoded.len() < 2 {
        return None;
    }
    // printpdf writes ColorTransform 0 into the stream dictionary, which would show YCbCr data
    // as RGB. An Adobe APP14 marker inside the JPEG takes precedence (PDF 32000, 7.4.8), so I
    // insert one that asks for the YCbCr to RGB transform.
    let adobe: [u8; 16] = [
        0xFF, 0xEE, 0x00, 0x0E, b'A', b'd', b'o', b'b', b'e', 0x00, 0x64, 0x00, 0x00, 0x00, 0x00,
        0x01,
    ];
    let mut data = Vec::with_capacity(encoded.len() + adobe.len());
    data.extend_from_slice(&encoded[..2]);
    data.extend_from_slice(&adobe);
    data.extend_from_slice(&encoded[2..]);
    Some(Image::from(ImageXObject {
        width: Px(width as usize),
        height: Px(height as usize),
        color_space: ColorSpace::Rgb,
        bits_per_component: ColorBits::Bit8,
        interpolate: true,
        image_data: data,
        image_filter: Some(ImageFilter::DCT),
        smask: None,
        clipping_bbox: None,
    }))
}

struct Writer<'a> {
    doc: PdfDocumentReference,
    layer: PdfLayerReference,
    face: ttf_parser::Face<'a>,
    bold_face: ttf_parser::Face<'a>,
    regular: IndirectFontRef,
    bold: IndirectFontRef,
    y: f32,
    page: u32,
    page_index: PdfPageIndex,
    bookmarked: HashSet<u32>,
    stamp: &'a PdfStamp,
    title: String,
    root: &'a Path,
    /// Decoded, rounded avatars by file and pixel size; I decode each photo once.
    avatars: HashMap<(String, u32), Option<DynamicImage>>,
}

impl Writer<'_> {
    fn width_mm(&self, text: &str, size_pt: f32, bold: bool) -> f32 {
        let face = if bold { &self.bold_face } else { &self.face };
        let units = f32::from(face.units_per_em());
        let advance: f32 = text
            .chars()
            .map(|ch| {
                face.glyph_index(ch)
                    .and_then(|glyph| face.glyph_hor_advance(glyph))
                    .map(f32::from)
                    .unwrap_or(units * 0.5)
            })
            .sum();
        advance / units * size_pt * PT_TO_MM
    }

    /// I keep characters the font can draw and drop emoji and other missing glyphs.
    fn printable(&self, text: &str) -> String {
        text.chars()
            .filter_map(|ch| match ch {
                '\n' | '\t' => Some(' '),
                '\u{200d}' | '\u{fe0f}' | '\u{fe0e}' => None,
                ch if ch.is_control() => None,
                ch if self.face.glyph_index(ch).is_some() => Some(ch),
                ch if (ch as u32) >= 0x1F000 || ('\u{2600}'..='\u{27BF}').contains(&ch) => None,
                _ => Some('?'),
            })
            .collect()
    }

    /// I shorten text with an ellipsis so it fits the given width.
    fn fit(&self, text: &str, size: f32, bold: bool, max_w: f32) -> String {
        let clean = self.printable(text);
        if self.width_mm(&clean, size, bold) <= max_w {
            return clean;
        }
        let mut out = String::new();
        for ch in clean.chars() {
            if self.width_mm(&format!("{out}{ch}…"), size, bold) > max_w {
                break;
            }
            out.push(ch);
        }
        out.push('…');
        out
    }

    fn wrap(&self, text: &str, size_pt: f32, bold: bool, max_w: f32) -> Vec<String> {
        let mut lines = Vec::new();
        for raw_line in text.split('\n') {
            let clean = self.printable(raw_line);
            let mut line = String::new();
            for word in clean.split(' ') {
                let candidate = if line.is_empty() {
                    word.to_string()
                } else {
                    format!("{line} {word}")
                };
                if self.width_mm(&candidate, size_pt, bold) <= max_w {
                    line = candidate;
                    continue;
                }
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                if self.width_mm(word, size_pt, bold) <= max_w {
                    line = word.to_string();
                } else {
                    for ch in word.chars() {
                        let next = format!("{line}{ch}");
                        if self.width_mm(&next, size_pt, bold) > max_w && !line.is_empty() {
                            lines.push(std::mem::take(&mut line));
                            line.push(ch);
                        } else {
                            line = next;
                        }
                    }
                }
            }
            lines.push(line);
        }
        lines
    }

    fn new_page(&mut self) {
        self.footer();
        let (page, layer) = self.doc.add_page(Mm(PAGE_W), Mm(PAGE_H), "content");
        self.layer = self.doc.get_page(page).get_layer(layer);
        self.page_index = page;
        self.page += 1;
        self.y = TOP;
        let title = self.fit(&self.title.clone(), 8.0, true, PAGE_W - 2.0 * MARGIN);
        self.text(&title, 8.0, true, GRAY, MARGIN, PAGE_H - 11.0);
        self.rule(PAGE_H - 13.0);
    }

    fn ensure(&mut self, height: f32) {
        if self.y - height < BOTTOM {
            self.new_page();
        }
    }

    /// I add an outline entry; the PDF outline keeps one entry per page, so the first one wins.
    fn bookmark(&mut self, name: &str) {
        if self.bookmarked.insert(self.page) {
            self.doc.add_bookmark(name, self.page_index);
        }
    }

    fn text(&self, text: &str, size: f32, bold: bool, shade: Shade, x: f32, y: f32) {
        self.layer.set_fill_color(color(shade));
        let font = if bold { &self.bold } else { &self.regular };
        self.layer.use_text(text, size, Mm(x), Mm(y), font);
    }

    fn text_right(&self, text: &str, size: f32, bold: bool, shade: Shade, right: f32, y: f32) {
        let width = self.width_mm(text, size, bold);
        self.text(text, size, bold, shade, right - width, y);
    }

    fn rule(&self, y: f32) {
        self.box_fill(MARGIN, y, PAGE_W - 2.0 * MARGIN, 0.25, RULE);
    }

    fn box_fill(&self, x: f32, y: f32, w: f32, h: f32, shade: Shade) {
        self.layer.set_fill_color(color(shade));
        self.layer
            .add_rect(Rect::new(Mm(x), Mm(y), Mm(x + w), Mm(y + h)).with_mode(PaintMode::Fill));
    }

    fn circle(&self, cx: f32, cy: f32, r: f32, shade: Shade) {
        self.layer.set_fill_color(color(shade));
        self.layer.add_polygon(Polygon {
            rings: vec![calculate_points_for_circle(Mm(r), Mm(cx), Mm(cy))],
            mode: PaintMode::Fill,
            winding_order: WindingOrder::NonZero,
        });
    }

    fn footer(&self) {
        let stamp = format!(
            "Requested {} · Completed {} · {} · page {}",
            local_time(self.stamp.requested_at, "%d.%m.%Y %H:%M:%S"),
            local_time(self.stamp.completed_at, "%d.%m.%Y %H:%M:%S"),
            self.printable(&self.stamp.account_name),
            self.page
        );
        self.text(&stamp, 7.0, false, GRAY, MARGIN, 9.0);
    }

    fn paragraph(&mut self, text: &str, size: f32, bold: bool, shade: Shade, x: f32) {
        self.paragraph_w(text, size, bold, shade, x, PAGE_W - MARGIN - x);
    }

    fn paragraph_w(&mut self, text: &str, size: f32, bold: bool, shade: Shade, x: f32, w: f32) {
        let step = (size * 0.46).max(LINE * 0.8);
        for line in self.wrap(text, size, bold, w) {
            self.ensure(step);
            self.y -= step;
            self.text(&line, size, bold, shade, x, self.y);
        }
    }

    fn heading(&mut self, title: &str) {
        self.ensure(22.0);
        self.y -= 9.0;
        self.text(title, 13.0, true, TEXT, MARGIN, self.y);
        self.y -= 2.8;
        self.rule(self.y);
        self.y -= 1.5;
    }

    fn avatar_image(&mut self, file: &str, px: u32) -> Option<DynamicImage> {
        let root = self.root;
        self.avatars
            .entry((file.to_string(), px))
            .or_insert_with(|| round_avatar(&root.join(file), px))
            .clone()
    }

    /// I draw a round photo, or a colored circle with the first letter when there is none.
    fn avatar(&mut self, file: Option<&str>, seed: i64, name: &str, x: f32, y: f32, size: f32) {
        let px = if size > 14.0 { 192 } else { 64 };
        let picture = file
            .and_then(|file| self.avatar_image(file, px))
            .and_then(|image| jpeg_image(&image, 88));
        if let Some(picture) = picture {
            picture.add_to_layer(
                self.layer.clone(),
                ImageTransform {
                    translate_x: Some(Mm(x)),
                    translate_y: Some(Mm(y)),
                    dpi: Some(px as f32 * 25.4 / size),
                    ..Default::default()
                },
            );
            return;
        }
        self.circle(x + size / 2.0, y + size / 2.0, size / 2.0, palette(seed));
        let initial = self
            .printable(name)
            .chars()
            .find(|ch| ch.is_alphanumeric())
            .map(|ch| ch.to_uppercase().collect::<String>())
            .unwrap_or_else(|| "?".into());
        let pt = size * 1.45;
        let width = self.width_mm(&initial, pt, true);
        let baseline = y + size / 2.0 - pt * PT_TO_MM * 0.36;
        self.text(
            &initial,
            pt,
            true,
            WHITE,
            x + (size - width) / 2.0,
            baseline,
        );
    }

    /// The name I show for a sender; my own messages carry the account name.
    fn sender_name(&self, chat: &ChatExport, message: &MessageExport) -> String {
        let mine =
            message.from_id == Some(self.stamp.self_id) || (message.out && chat.kind != "channel");
        if mine {
            return self.stamp.account_name.clone();
        }
        message
            .from_name
            .clone()
            .or_else(|| {
                message
                    .from_id
                    .and_then(|id| chat.participants.get(&id))
                    .map(|p| p.name.clone())
            })
            .unwrap_or_else(|| chat.title.clone())
    }

    fn sender_avatar(&self, chat: &ChatExport, message: &MessageExport) -> Option<String> {
        let id = message
            .from_id
            .or_else(|| (message.out && chat.kind != "channel").then_some(self.stamp.self_id))?;
        chat.participants
            .get(&id)
            .and_then(|p| p.avatar.clone())
            .or_else(|| (id == chat.id).then(|| chat.avatar.clone()).flatten())
    }

    fn cover(&mut self, chat: &ChatExport) {
        self.bookmark("Overview");
        let size = 26.0;
        let top = self.y - 2.0;
        self.avatar(
            chat.avatar.as_deref(),
            chat.id,
            &chat.title,
            MARGIN,
            top - size,
            size,
        );
        let x = MARGIN + size + 7.0;
        self.y = top + 1.5;
        self.paragraph(&chat.title, 20.0, true, TEXT, x);
        self.y -= 1.0;
        let subtitle = [
            chat.username.as_ref().map(|u| format!("@{u}")),
            Some(capitalize(&chat.kind)),
            chat.members.map(|n| format!("{n} members")),
            Some(format!("ID {}", chat.id)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        self.paragraph(&subtitle, 10.0, false, GRAY, x);
        if let Some(about) = &chat.about {
            self.y -= 1.5;
            self.paragraph(about, 9.5, false, TEXT, x);
        }
        self.y = self.y.min(top - size) - 7.0;

        let stats = &chat.stats;
        let media_saved = chat
            .messages
            .iter()
            .filter(|m| m.media.as_ref().is_some_and(|x| x.file.is_some()))
            .count();
        let media_total: usize = stats.media.values().sum();
        let flags = [
            chat.pinned.then_some("pinned"),
            chat.archived.then_some("archived"),
            chat.muted.then_some("muted"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let reply = |secs: Option<i64>| {
            secs.map(|s| human_duration(s as f64))
                .unwrap_or_else(|| "—".into())
        };
        let rows: Vec<(&str, String)> = vec![
            ("Chat type", capitalize(&chat.kind)),
            ("Chat ID", chat.id.to_string()),
            (
                "Username",
                chat.username
                    .as_ref()
                    .map(|u| format!("@{u}"))
                    .unwrap_or_else(|| "—".into()),
            ),
            (
                "Members",
                chat.members
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| chat.participants.len().to_string()),
            ),
            ("Account", self.stamp.account_name.clone()),
            ("Messages", stats.total.to_string()),
            (
                "Outgoing / incoming",
                format!("{} / {}", stats.outgoing, stats.incoming),
            ),
            ("Words", stats.words.to_string()),
            ("Media", format!("{media_total} ({media_saved} saved)")),
            ("Edited", stats.edited.to_string()),
            ("Forwarded", stats.forwarded.to_string()),
            ("Replies", stats.replies.to_string()),
            ("Deleted (restored)", stats.deleted.to_string()),
            ("Service messages", stats.service.to_string()),
            ("Active days", stats.active_days.to_string()),
            ("Unread", chat.unread.to_string()),
            ("My median reply", reply(stats.my_reply_secs)),
            ("Their median reply", reply(stats.their_reply_secs)),
            (
                "First message",
                stats
                    .first_date
                    .map(|d| local_time(d, "%d.%m.%Y %H:%M"))
                    .unwrap_or_else(|| "—".into()),
            ),
            (
                "Last message",
                stats
                    .last_date
                    .map(|d| local_time(d, "%d.%m.%Y %H:%M"))
                    .unwrap_or_else(|| "—".into()),
            ),
            (
                "Export requested",
                local_time(self.stamp.requested_at, "%d.%m.%Y %H:%M:%S"),
            ),
            (
                "Export completed",
                local_time(self.stamp.completed_at, "%d.%m.%Y %H:%M:%S"),
            ),
            (
                "Dialog flags",
                if flags.is_empty() {
                    "—".into()
                } else {
                    flags.join(", ")
                },
            ),
        ];
        let per_column = rows.len().div_ceil(2);
        let row_h = 5.6;
        let height = per_column as f32 * row_h + 6.0;
        self.ensure(height + 4.0);
        let top = self.y;
        let full = PAGE_W - 2.0 * MARGIN;
        self.box_fill(MARGIN, top - height, full, height, SOFT);
        let col_w = full / 2.0;
        for (i, (label, value)) in rows.iter().enumerate() {
            let col = (i / per_column) as f32;
            let row = (i % per_column) as f32;
            let x = MARGIN + 5.0 + col * col_w;
            let y = top - 6.0 - row * row_h;
            self.text(label, 8.5, false, GRAY, x, y);
            let value = self.fit(value, 8.5, true, col_w - 42.0);
            self.text(&value, 8.5, true, TEXT, x + 36.0, y);
        }
        self.y = top - height - 5.0;
        let legend = "Left bars: green marks your messages, red marks deleted messages restored from the anti-delete archive. Times use the clock of the computer that made the export.";
        self.paragraph(legend, 7.5, false, GRAY, MARGIN);
    }

    fn participants(&mut self, chat: &ChatExport) {
        let mut counts: HashMap<i64, usize> = HashMap::new();
        for message in chat.messages.iter().filter(|m| m.service.is_none()) {
            let id = message
                .from_id
                .or_else(|| (message.out && chat.kind != "channel").then_some(self.stamp.self_id));
            if let Some(id) = id {
                *counts.entry(id).or_default() += 1;
            }
        }
        let mut ids = chat.participants.keys().copied().collect::<Vec<_>>();
        for id in counts.keys() {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }
        if ids.is_empty() {
            return;
        }
        ids.sort_by_key(|id| std::cmp::Reverse(counts.get(id).copied().unwrap_or(0)));
        let total = counts.values().sum::<usize>().max(1);

        self.heading(&format!("Participants ({})", ids.len()));
        self.bookmark("Participants");
        let shown = ids.len().min(MAX_PARTICIPANT_ROWS);
        for id in &ids[..shown] {
            let participant = chat.participants.get(id);
            let name = if *id == self.stamp.self_id {
                format!("{} (you)", self.stamp.account_name)
            } else {
                participant
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| id.to_string())
            };
            let avatar = participant
                .and_then(|p| p.avatar.clone())
                .or_else(|| (*id == chat.id).then(|| chat.avatar.clone()).flatten());
            let row_h = 11.0;
            self.ensure(row_h);
            let top = self.y - 1.5;
            let size = 8.5;
            self.avatar(avatar.as_deref(), *id, &name, MARGIN, top - size, size);
            let x = MARGIN + size + 4.0;
            let name_w = 92.0;
            let fitted = self.fit(&name, 10.0, true, name_w);
            self.text(&fitted, 10.0, true, palette(*id), x, top - 3.4);
            let details = [
                participant
                    .and_then(|p| p.username.as_ref())
                    .map(|u| format!("@{u}")),
                Some(format!("ID {id}")),
                participant.filter(|p| p.bot).map(|_| "bot".to_string()),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            let details = self.fit(&details, 8.0, false, name_w);
            self.text(&details, 8.0, false, GRAY, x, top - 7.4);

            let count = counts.get(id).copied().unwrap_or(0);
            let share = count as f32 / total as f32;
            let bar_x = PAGE_W - MARGIN - 52.0;
            let bar_w = 30.0;
            self.box_fill(bar_x, top - 5.6, bar_w, 2.2, SOFT);
            if count > 0 {
                self.box_fill(
                    bar_x,
                    top - 5.6,
                    (bar_w * share).max(0.6),
                    2.2,
                    palette(*id),
                );
            }
            let label = format!("{count} · {:.0}%", share * 100.0);
            self.text_right(&label, 8.5, true, TEXT, PAGE_W - MARGIN, top - 5.0);
            self.y -= row_h;
        }
        if ids.len() > shown {
            self.paragraph(
                &format!("…and {} more", ids.len() - shown),
                8.5,
                false,
                GRAY,
                MARGIN,
            );
        }
    }

    fn bar_chart(&mut self, title: &str, values: &[usize], labels: &[String]) {
        let chart_h = 24.0;
        self.ensure(chart_h + 16.0);
        self.y -= 6.0;
        self.text(title, 9.5, true, GRAY, MARGIN, self.y);
        self.y -= 4.0;
        let base = self.y - chart_h;
        let max = values.iter().copied().max().unwrap_or(0).max(1);
        let width = PAGE_W - 2.0 * MARGIN;
        let slot = width / values.len().max(1) as f32;
        let bar_w = slot * 0.68;
        self.box_fill(MARGIN, base - 0.25, width, 0.25, RULE);
        for (i, &value) in values.iter().enumerate() {
            let x = MARGIN + slot * i as f32 + (slot - bar_w) / 2.0;
            if value > 0 {
                let h = (chart_h * value as f32 / max as f32).max(0.5);
                let shade = if value == max {
                    ACCENT
                } else {
                    (0.6, 0.74, 0.93)
                };
                self.box_fill(x, base, bar_w, h, shade);
                if value == max {
                    let label = value.to_string();
                    let w = self.width_mm(&label, 7.0, true);
                    self.text(
                        &label,
                        7.0,
                        true,
                        TEXT,
                        x + (bar_w - w) / 2.0,
                        base + h + 1.0,
                    );
                }
            }
            if let Some(label) = labels.get(i).filter(|l| !l.is_empty()) {
                let w = self.width_mm(label, 7.0, false);
                self.text(label, 7.0, false, GRAY, x + (bar_w - w) / 2.0, base - 3.6);
            }
        }
        self.y = base - 6.0;
    }

    fn share_rows(&mut self, title: &str, rows: &[(String, usize)]) {
        if rows.is_empty() {
            return;
        }
        self.ensure(14.0);
        self.y -= 6.0;
        self.text(title, 9.5, true, GRAY, MARGIN, self.y);
        self.y -= 1.0;
        let max = rows.iter().map(|r| r.1).max().unwrap_or(1).max(1);
        for (label, value) in rows {
            self.ensure(5.5);
            self.y -= 5.0;
            let label = self.fit(&capitalize(label), 8.5, false, 44.0);
            self.text(&label, 8.5, false, TEXT, MARGIN, self.y);
            let bar_x = MARGIN + 48.0;
            let bar_w = 100.0 * *value as f32 / max as f32;
            self.box_fill(bar_x, self.y - 0.2, bar_w.max(0.6), 2.6, ACCENT);
            self.text(
                &value.to_string(),
                8.0,
                true,
                GRAY,
                bar_x + bar_w + 2.0,
                self.y,
            );
        }
    }

    fn statistics(&mut self, chat: &ChatExport) {
        let stats = &chat.stats;
        if stats.total == 0 {
            return;
        }
        self.heading("Activity");
        self.bookmark("Activity");
        let hours = (0..24)
            .map(|h| {
                if h % 3 == 0 {
                    format!("{h:02}")
                } else {
                    String::new()
                }
            })
            .collect::<Vec<_>>();
        self.bar_chart("Messages by hour of day", &stats.by_hour, &hours);
        let days = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>();
        self.bar_chart("Messages by day of week", &stats.by_weekday, &days);

        let mut media = stats
            .media
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect::<Vec<_>>();
        media.sort_by_key(|r| std::cmp::Reverse(r.1));
        self.share_rows("Media by type", &media);

        let mut reactions = stats
            .reactions
            .iter()
            .filter_map(|(emoji, count)| {
                let label = self.printable(emoji);
                (!label.trim().is_empty()).then_some((label, *count as usize))
            })
            .collect::<Vec<_>>();
        reactions.sort_by_key(|r| std::cmp::Reverse(r.1));
        reactions.truncate(10);
        self.share_rows("Reactions", &reactions);
        let total_reactions: i64 = stats.reactions.values().sum();
        if total_reactions > 0 {
            self.y -= 1.5;
            self.paragraph(
                &format!(
                    "{total_reactions} reactions in total across {} kinds.",
                    stats.reactions.len()
                ),
                8.0,
                false,
                GRAY,
                MARGIN,
            );
        }

        let mut months: Vec<(String, usize)> = Vec::new();
        for message in &chat.messages {
            let month = local_time(message.date, "%Y-%m");
            match months.last_mut() {
                Some((last, count)) if *last == month => *count += 1,
                _ => months.push((month, 1)),
            }
        }
        if months.len() > 1 {
            let recent = &months[months.len().saturating_sub(24)..];
            let values = recent.iter().map(|m| m.1).collect::<Vec<_>>();
            let step = recent.len().div_ceil(8).max(1);
            let labels = recent
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    if i % step == 0 {
                        m.0.get(2..).unwrap_or(&m.0).replace('-', ".")
                    } else {
                        String::new()
                    }
                })
                .collect::<Vec<_>>();
            self.bar_chart("Messages by month", &values, &labels);
        }
    }

    fn messages(&mut self, chat: &ChatExport) {
        self.new_page();
        self.bookmark("Messages");
        if chat.messages.is_empty() {
            self.paragraph(
                "No messages match the export rules.",
                BODY_PT,
                false,
                GRAY,
                MARGIN,
            );
            return;
        }
        let by_id: HashMap<i32, &MessageExport> = chat.messages.iter().map(|m| (m.id, m)).collect();
        let mut last_day = String::new();
        let mut last_month = String::new();
        let mut previous: Option<(Option<i64>, bool, i64)> = None;
        for message in &chat.messages {
            let day = local_time(message.date, "%d %B %Y");
            if day != last_day {
                self.day(&day);
                let month = local_time(message.date, "%B %Y");
                if month != last_month {
                    self.bookmark(&month);
                    last_month = month;
                }
                last_day = day;
                previous = None;
            }
            if message.service.is_some() {
                self.service(message, chat);
                previous = None;
                continue;
            }
            let sender = (message.from_id, message.out);
            let head = !previous.is_some_and(|(from, out, date)| {
                (from, out) == sender && message.date - date < GROUP_GAP_SECS
            });
            self.message(message, chat, head, &by_id);
            previous = Some((sender.0, sender.1, message.date));
        }
    }

    fn day(&mut self, day: &str) {
        self.ensure(16.0);
        self.y -= 9.0;
        let width = self.width_mm(day, 9.0, true);
        let x = (PAGE_W - width) / 2.0;
        self.box_fill(x - 4.0, self.y - 1.6, width + 8.0, 5.6, SOFT);
        self.text(day, 9.0, true, GRAY, x, self.y);
        self.y -= 2.0;
    }

    fn service(&mut self, message: &MessageExport, chat: &ChatExport) {
        let action = message.service.as_deref().unwrap_or_default();
        let who = message
            .from_id
            .map(|_| self.sender_name(chat, message))
            .map(|n| format!("{n}: "))
            .unwrap_or_default();
        let line = format!(
            "{who}{action} · {} · #{}",
            local_time(message.date, "%H:%M"),
            message.id
        );
        self.y -= 1.5;
        self.paragraph(&line, 8.5, false, GRAY, MSG_X);
    }

    fn message(
        &mut self,
        message: &MessageExport,
        chat: &ChatExport,
        head: bool,
        by_id: &HashMap<i32, &MessageExport>,
    ) {
        let x = MSG_X;
        let right = PAGE_W - MARGIN;
        let width = right - x - TIME_COL;
        let time = format!("{} · #{}", local_time(message.date, "%H:%M:%S"), message.id);
        if head {
            self.ensure(MSG_AVATAR + 6.0);
            self.y -= 3.5;
        } else {
            self.ensure(LINE * 2.0);
            self.y -= 0.8;
        }
        let start = self.y;
        if head {
            let name = self.sender_name(chat, message);
            let seed = message.from_id.unwrap_or(if message.out {
                self.stamp.self_id
            } else {
                chat.id
            });
            let avatar = self.sender_avatar(chat, message);
            self.y -= LINE;
            self.avatar(
                avatar.as_deref(),
                seed,
                &name,
                MARGIN,
                self.y + 3.4 - MSG_AVATAR,
                MSG_AVATAR,
            );
            let name_shade = if message.out { MINE } else { palette(seed) };
            let fitted = self.fit(&name, 9.5, true, width - 30.0);
            self.text(&fitted, 9.5, true, name_shade, x, self.y);
            let username = message
                .from_id
                .and_then(|id| chat.participants.get(&id))
                .and_then(|p| p.username.as_ref())
                .map(|u| format!("@{u}"));
            if let Some(username) = username {
                let offset = self.width_mm(&fitted, 9.5, true) + 2.0;
                let left = width - offset;
                if left > 12.0 {
                    let username = self.fit(&username, 8.0, false, left);
                    self.text(&username, 8.0, false, GRAY, x + offset, self.y);
                }
            }
            self.text_right(&time, 7.0, false, GRAY, right, self.y);
        } else {
            // A message in a sender block shows its time on its own first line.
            self.text_right(&time, 7.0, false, GRAY, right, self.y - LINE);
        }

        if let Some(mark) = &message.deleted {
            let line = format!(
                "Deleted {} (restored from the anti-delete archive)",
                local_time(mark.deleted_at, "%d.%m.%Y %H:%M")
            );
            self.paragraph_w(&line, 8.5, true, DANGER, x, width);
        }
        if let Some(forwarded) = &message.forwarded_from {
            self.paragraph_w(
                &format!("Forwarded from {forwarded}"),
                8.5,
                false,
                GRAY,
                x,
                width,
            );
        }
        if let Some(reply) = message.reply_to {
            let snippet = by_id
                .get(&reply)
                .map(|m| {
                    let text = if m.text.is_empty() {
                        m.media
                            .as_ref()
                            .map(|media| format!("[{}]", media.type_name))
                            .unwrap_or_default()
                    } else {
                        m.text.chars().take(90).collect()
                    };
                    format!("{}: {}", self.sender_name(chat, m), text)
                })
                .unwrap_or_else(|| format!("message #{reply}"));
            let top = self.y;
            self.paragraph_w(
                &format!("In reply to {snippet}"),
                8.0,
                false,
                GRAY,
                x + 2.5,
                width - 2.5,
            );
            if self.y < top {
                self.box_fill(x, self.y - 0.8, 0.6, top - self.y, ACCENT);
            }
        }
        if let Some(media) = &message.media {
            let mut drawn = false;
            if let (Some(file), true) = (
                &media.file,
                matches!(media.kind, Some(MediaKind::Photo)) || media.is_visual(),
            ) {
                drawn = self.image(&self.root.join(file), x);
            }
            let details = [
                media.name.clone(),
                media.details.clone(),
                media
                    .width
                    .zip(media.height)
                    .map(|(w, h)| format!("{w}×{h}")),
                media.duration.map(human_duration),
                media.size.map(human_bytes),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            let mut label = capitalize(&media.type_name);
            if !details.is_empty() {
                label = format!("{label}: {details}");
            }
            let note = match (&media.file, &media.skipped) {
                (Some(file), _) => format!("Saved as {file}"),
                (None, Some(reason)) => reason.clone(),
                (None, None) => String::new(),
            };
            self.y -= 0.6;
            let top = self.y;
            self.paragraph_w(
                &format!("[{label}]"),
                8.0,
                false,
                GRAY,
                x + 2.5,
                width - 2.5,
            );
            if !drawn && !note.is_empty() {
                self.paragraph_w(&note, 7.5, false, GRAY, x + 2.5, width - 2.5);
            }
            if self.y < top {
                self.box_fill(x, self.y - 0.8, 0.6, top - self.y, GRAY);
            }
        }
        if !message.text.is_empty() {
            self.paragraph_w(&message.text, BODY_PT, false, TEXT, x, width);
        }
        if !message.reactions.is_empty() {
            let line = message
                .reactions
                .iter()
                .map(|r| {
                    let emoji = self.printable(&r.emoji);
                    let emoji = if emoji.trim().is_empty() {
                        "reaction".to_string()
                    } else {
                        emoji
                    };
                    let mine = if r.chosen { " (mine)" } else { "" };
                    format!("{emoji} ×{}{mine}", r.count)
                })
                .collect::<Vec<_>>()
                .join("   ");
            self.paragraph_w(&format!("Reactions: {line}"), 7.5, false, GRAY, x, width);
        }
        let meta = [
            message
                .edited
                .map(|e| format!("edited {}", local_time(e, "%d.%m.%Y %H:%M"))),
            match message.read {
                Some(true) => Some("read".to_string()),
                Some(false) => Some("unread".to_string()),
                None => None,
            },
            message.pinned.then(|| "pinned".to_string()),
            message.via_bot.map(|id| {
                chat.participants
                    .get(&id)
                    .and_then(|p| p.username.as_ref())
                    .map(|u| format!("via @{u}"))
                    .unwrap_or_else(|| format!("via bot {id}"))
            }),
            message.views.map(|v| format!("{v} views")),
            message
                .forwards
                .filter(|f| *f > 0)
                .map(|f| format!("{f} forwards")),
            message.grouped_id.map(|_| "album".to_string()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        if !meta.is_empty() {
            self.paragraph_w(&meta.join(" · "), 7.0, false, GRAY, x, width);
        }

        let bar = if message.deleted.is_some() {
            Some(DANGER)
        } else if message.out {
            Some(MINE)
        } else {
            None
        };
        if let Some(shade) = bar {
            if start > self.y && self.y > BOTTOM - 1.0 {
                self.box_fill(x - 2.2, self.y - 0.8, 0.9, start - self.y - 0.4, shade);
            }
        }
    }

    fn image(&mut self, path: &Path, x: f32) -> bool {
        let Ok(image) = image_crate::open(path) else {
            return false;
        };
        let image = if image.width().max(image.height()) > MAX_IMAGE_PX {
            image.thumbnail(MAX_IMAGE_PX, MAX_IMAGE_PX)
        } else {
            image
        };
        let (px_w, px_h) = image.dimensions();
        if px_w == 0 || px_h == 0 {
            return false;
        }
        let Some(picture) = jpeg_image(&image, 82) else {
            return false;
        };
        let scale = (MAX_IMAGE_W / px_w as f32).min(MAX_IMAGE_H / px_h as f32);
        let width_mm = px_w as f32 * scale;
        let height_mm = px_h as f32 * scale;
        self.ensure(height_mm + 3.0);
        self.y -= height_mm + 1.5;
        let dpi = px_w as f32 * 25.4 / width_mm;
        picture.add_to_layer(
            self.layer.clone(),
            ImageTransform {
                translate_x: Some(Mm(x)),
                translate_y: Some(Mm(self.y)),
                dpi: Some(dpi),
                ..Default::default()
            },
        );
        self.y -= 1.0;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::model::{ChatStats, DeletedMark, MediaExport, Participant, ReactionExport};

    #[test]
    fn renders_a_small_chat() {
        let dir = std::env::temp_dir().join(format!("fly_pdf_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("avatars")).unwrap();
        let photo = RgbImage::from_fn(120, 90, |x, y| Pixel([(x * 2) as u8, (y * 2) as u8, 180]));
        photo.save(dir.join("avatars/2.jpg")).unwrap();

        let mut chat = ChatExport {
            id: 2,
            title: "Тест чат 😀".into(),
            kind: "private".into(),
            username: Some("friend".into()),
            avatar: Some("avatars/2.jpg".into()),
            ..ChatExport::default()
        };
        chat.participants.insert(
            2,
            Participant {
                name: "Друг".into(),
                username: Some("friend".into()),
                avatar: Some("avatars/2.jpg".into()),
                bot: false,
            },
        );
        chat.participants.insert(
            1,
            Participant {
                name: "Saved Messages".into(),
                ..Participant::default()
            },
        );
        chat.messages = (0..120)
            .map(|i| MessageExport {
                id: i,
                date: 1_700_000_000 + i64::from(i) * 4000,
                out: i % 3 == 0,
                from_id: Some(if i % 3 == 0 { 1 } else { 2 }),
                text: format!("Сообщение номер {i} с довольно длинным текстом, чтобы проверить перенос строк по ширине страницы ✨"),
                deleted: (i == 5).then_some(DeletedMark { deleted_at: 1_700_100_000, restored_from_archive: true }),
                reply_to: (i == 9).then_some(4),
                edited: (i == 11).then_some(1_700_050_000),
                reactions: if i == 12 {
                    vec![ReactionExport { emoji: "❤".into(), count: 2, chosen: true }]
                } else {
                    Vec::new()
                },
                media: (i == 7).then(|| MediaExport {
                    type_name: "video".into(),
                    kind: Some(MediaKind::Video),
                    skipped: Some("Video excluded by export rules".into()),
                    ..MediaExport::default()
                }),
                ..MessageExport::default()
            })
            .collect();
        chat.stats = ChatStats::build(&chat);
        let bytes = render_chat(
            &chat,
            &dir,
            &PdfStamp {
                account_name: "Alex".into(),
                self_id: 1,
                requested_at: 1_700_000_000,
                completed_at: 1_700_000_100,
            },
        )
        .unwrap();
        if let Ok(path) = std::env::var("FLY_PDF_PREVIEW") {
            std::fs::write(path, &bytes).unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(bytes.starts_with(b"%PDF"));
        assert!(bytes.len() > 10_000);
    }

    #[test]
    fn avatars_are_round_on_white() {
        let dir = std::env::temp_dir().join(format!("fly_avatar_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.png");
        RgbImage::from_pixel(50, 80, Pixel([0, 0, 0]))
            .save(&path)
            .unwrap();
        let round = round_avatar(&path, 40).unwrap().to_rgb8();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(round.dimensions(), (40, 40));
        assert_eq!(round.get_pixel(0, 0), &Pixel([255, 255, 255]));
        assert_eq!(round.get_pixel(20, 20), &Pixel([0, 0, 0]));
    }
}
