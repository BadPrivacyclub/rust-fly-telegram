//! Personas: I swap the account's public face (name, bio, username and public photo) for a
//! generated one and put everything back on request.
//!
//! I never touch the existing profile photos. I hide them by setting "Who can see my profile
//! photo" to Nobody and show the generated picture as the public (fallback) photo instead.
//! Before changing anything I save the original privacy rules with every exception, so a
//! restore brings back exactly what was there.
//!
//! While a persona is active the old username is free, so I park it on a private channel of
//! the account. On restore I make that channel private again and keep it for next time.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use grammers_client::media::Downloadable;
use grammers_client::{tl, Client};
use grammers_mtsender::InvocationError;
use printpdf::image_crate::{self, imageops::FilterType, DynamicImage, GenericImageView};
use rand::seq::SliceRandom;
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::account_tools::{now_secs, AccountHandle};
use crate::database::Database;
use crate::i18n::Lang;
use crate::loader::http;

const STATE_PREFIX: &str = "persona.state.";
const RESERVE_PREFIX: &str = "persona.reserve.";
const DATA_DIR: &str = "data/persona";
const PHOTO_PX: u32 = 640;
const IMAGE_LIMIT: usize = 12 * 1024 * 1024;
const USERNAME_ATTEMPTS: usize = 15;
const RESERVE_TITLE: &str = "Reserved";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Anime,
    Tiktok,
    Osint,
    Anon,
    Music,
    Art,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Anime,
        Kind::Tiktok,
        Kind::Osint,
        Kind::Anon,
        Kind::Music,
        Kind::Art,
    ];

    /// I accept English and Russian names; "random" picks any persona.
    pub fn parse(value: &str) -> Option<Kind> {
        let value = value.trim().to_lowercase();
        let kind = match value.as_str() {
            "anime" | "weeb" | "otaku" | "аниме" | "анимешник" | "анимешница" => {
                Kind::Anime
            }
            "tiktok" | "tiktoker" | "tt" | "тикток" | "тиктокер" | "тиктокерша" => {
                Kind::Tiktok
            }
            "osint" | "osinter" | "осинт" | "осинтер" => Kind::Osint,
            "anon" | "anonymous" | "аноним" | "анон" => Kind::Anon,
            "music" | "musician" | "музыка" | "музыкант" => Kind::Music,
            "art" | "artist" | "художник" | "художница" | "арт" => Kind::Art,
            "random" | "rand" | "случайно" | "рандом" | "" => {
                *Kind::ALL.choose(&mut rand::thread_rng())?
            }
            _ => return None,
        };
        Some(kind)
    }

    pub fn key(self) -> &'static str {
        match self {
            Kind::Anime => "anime",
            Kind::Tiktok => "tiktok",
            Kind::Osint => "osint",
            Kind::Anon => "anon",
            Kind::Music => "music",
            Kind::Art => "art",
        }
    }

    pub fn title(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Kind::Anime, Lang::Ru) => "Анимешник",
            (Kind::Tiktok, Lang::Ru) => "Тиктокер",
            (Kind::Osint, Lang::Ru) => "Осинтер",
            (Kind::Anon, Lang::Ru) => "Аноним",
            (Kind::Music, Lang::Ru) => "Музыкант",
            (Kind::Art, Lang::Ru) => "Художник",
            (Kind::Anime, Lang::En) => "Anime fan",
            (Kind::Tiktok, Lang::En) => "TikToker",
            (Kind::Osint, Lang::En) => "OSINT analyst",
            (Kind::Anon, Lang::En) => "Anonymous",
            (Kind::Music, Lang::En) => "Musician",
            (Kind::Art, Lang::En) => "Artist",
        }
    }

    pub fn emoji(self) -> &'static str {
        match self {
            Kind::Anime => "🌸",
            Kind::Tiktok => "📱",
            Kind::Osint => "🔎",
            Kind::Anon => "🕶",
            Kind::Music => "🎧",
            Kind::Art => "🎨",
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Identity generation
// ---------------------------------------------------------------------------------------------

/// A generated name, username and bio.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Identity {
    pub first_name: String,
    pub last_name: String,
    pub about: String,
}

fn pick<'a>(rng: &mut impl Rng, items: &[&'a str]) -> &'a str {
    items.choose(rng).copied().unwrap_or_default()
}

fn digits(rng: &mut impl Rng) -> String {
    match rng.gen_range(0..4) {
        0 => String::new(),
        1 => rng.gen_range(1..10).to_string(),
        2 => rng.gen_range(10..100).to_string(),
        _ => rng.gen_range(2000..2011).to_string(),
    }
}

fn random_tail(rng: &mut impl Rng, len: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    (0..len)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

/// I bring a candidate to Telegram's rules: 5-32 characters of a-z, 0-9 and single
/// underscores, starting with a letter and not ending with an underscore.
pub fn clean_username(raw: &str) -> Option<String> {
    let mut out = String::new();
    for ch in raw.to_lowercase().chars() {
        let ch = match ch {
            'a'..='z' | '0'..='9' => ch,
            _ => '_',
        };
        if ch == '_' && (out.is_empty() || out.ends_with('_')) {
            continue;
        }
        out.push(ch);
    }
    let mut out = out
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '_')
        .to_string();
    out.truncate(32);
    let out = out.trim_end_matches('_').to_string();
    (5..=32).contains(&out.len()).then_some(out)
}

fn username_candidate(kind: Kind, identity: &Identity, rng: &mut impl Rng) -> String {
    let first = identity.first_name.to_lowercase();
    let word = first
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find(|w| w.len() >= 3)
        .unwrap_or("user")
        .to_string();
    let n = digits(rng);
    let raw = match kind {
        Kind::Anime => {
            let prefix = pick(
                rng,
                &[
                    "", "neko_", "kawaii_", "otaku_", "yume_", "hoshi_", "tsuki_", "sakura_",
                ],
            );
            let suffix = pick(
                rng,
                &["_chan", "_kun", "_san", "_desu", "_uwu", "", "_senpai"],
            );
            format!("{prefix}{word}{suffix}{n}")
        }
        Kind::Tiktok => match rng.gen_range(0..5) {
            0 => format!("itz_{word}{n}"),
            1 => format!("{word}_vibes{n}"),
            2 => format!("official_{word}{n}"),
            3 => format!("{word}_tok{n}"),
            _ => format!("{word}xo{n}"),
        },
        Kind::Osint => {
            let tail = pick(
                rng,
                &[
                    "intel", "recon", "trace", "osint", "lookup", "sigint", "watch",
                ],
            );
            match rng.gen_range(0..3) {
                0 => format!("{word}_{tail}{n}"),
                1 => format!("{tail}_{word}{n}"),
                _ => format!("{word}{tail}_{}", random_tail(rng, 3)),
            }
        }
        Kind::Anon => match rng.gen_range(0..3) {
            0 => format!("anon_{}", random_tail(rng, 6)),
            1 => format!("x{}", random_tail(rng, 8)),
            _ => format!("nobody_{}", random_tail(rng, 5)),
        },
        Kind::Music => {
            let tail = pick(
                rng,
                &[
                    "beats", "tapes", "sound", "wave", "music", "audio", "records",
                ],
            );
            match rng.gen_range(0..3) {
                0 => format!("{word}_{tail}{n}"),
                1 => format!("{tail}by_{word}{n}"),
                _ => format!("{word}{tail}{n}"),
            }
        }
        Kind::Art => {
            let tail = pick(
                rng,
                &[
                    "art", "draws", "paints", "studio", "sketch", "atelier", "ink",
                ],
            );
            match rng.gen_range(0..3) {
                0 => format!("{word}_{tail}{n}"),
                1 => format!("{tail}_by_{word}{n}"),
                _ => format!("{word}{tail}{n}"),
            }
        }
    };
    clean_username(&raw).unwrap_or_else(|| format!("user_{}", random_tail(rng, 7)))
}

pub fn generate_identity(kind: Kind, lang: Lang, rng: &mut impl Rng) -> Identity {
    let ru = lang == Lang::Ru;
    let (first_name, last_name, about) = match kind {
        Kind::Anime => {
            let name = pick(
                rng,
                &[
                    "Sakura", "Haruki", "Yuki", "Ren", "Aoi", "Hikari", "Kaito", "Mio", "Sora",
                    "Rin", "Akira", "Hinata", "Kenji", "Yume", "Shiro", "Kuro", "Natsu", "Haru",
                    "Kira", "Riku", "Asuka", "Kaede", "Mikasa", "Tsuki",
                ],
            );
            let suffix = pick(rng, &["", "", "-chan", "-kun", " senpai", "-san"]);
            let decor = pick(rng, &["🌸", "✨", "🍡", "🌙", "⛩", "🎴", "💮", "🍥"]);
            let bio = if ru {
                pick(
                    rng,
                    &[
                        "Смотрю аниме вместо сна 🌙",
                        "Сенпай однажды заметил меня ✨",
                        "Уже давно в исекае 🌸",
                        "Рамен, манга и никакого сна 🍜",
                        "Жду новый сезон с 2019 года ⏳",
                        "Мой вайфу лучше твоего 💮",
                    ],
                )
            } else {
                pick(
                    rng,
                    &[
                        "Watching anime instead of sleeping 🌙",
                        "Senpai noticed me once ✨",
                        "Already isekai'd 🌸",
                        "Ramen, manga and no sleep 🍜",
                        "Waiting for season 2 since 2019 ⏳",
                    ],
                )
            };
            (
                format!("{name}{suffix}"),
                decor.to_string(),
                bio.to_string(),
            )
        }
        Kind::Tiktok => {
            let name = if ru {
                pick(
                    rng,
                    &[
                        "Mila", "Alina", "Vika", "Liza", "Dasha", "Nika", "Milana", "Sasha", "Max",
                        "Mark", "Tim", "Kira", "Lera", "Sonya",
                    ],
                )
            } else {
                pick(
                    rng,
                    &[
                        "Mia", "Lexi", "Ava", "Chloe", "Kayla", "Max", "Leo", "Tyler", "Jayden",
                        "Bella", "Nova", "Zoe",
                    ],
                )
            };
            let decor = pick(rng, &["✨", "💫", "🎀", "🦋", "💅", "🔥", "📸", "🫶"]);
            let shape = rng.gen_range(0..3);
            let first = match shape {
                0 => format!("itz {name}"),
                1 => name.to_lowercase(),
                _ => name.to_string(),
            };
            let bio = if ru {
                pick(
                    rng,
                    &[
                        "📍 в рекомендациях",
                        "daily влоги ✨ коллаб в лс",
                        "делаю тренды раньше трендов 💫",
                        "1M мечтаю, 10k уже есть 🎀",
                        "снимаю жизнь в 9:16 📸",
                    ],
                )
            } else {
                pick(
                    rng,
                    &[
                        "📍 on your fyp",
                        "daily vlogs ✨ dm for collabs",
                        "trends before they trend 💫",
                        "living life in 9:16 📸",
                        "road to 1M 🎀",
                    ],
                )
            };
            (first, decor.to_string(), bio.to_string())
        }
        Kind::Osint => {
            let first = pick(
                rng,
                &[
                    "Shadow", "Trace", "Vector", "Cipher", "Ghost", "Nyx", "Raven", "Specter",
                    "Echo", "Hex", "Atlas", "Kestrel", "Onyx", "Vanta",
                ],
            );
            let last = pick(
                rng,
                &[
                    "Recon", "Intel", "Watcher", "Analyst", "Seeker", "", "🔎", "OSINT",
                ],
            );
            let bio = if ru {
                pick(
                    rng,
                    &[
                        "Всё публичное — не секрет 🔎",
                        "Открытые источники. Закрытый рот.",
                        "Нахожу то, что вы забыли удалить",
                        "Метаданные не врут 🛰",
                        "Геолокация по тени — хобби",
                    ],
                )
            } else {
                pick(
                    rng,
                    &[
                        "Open sources. Closed mouth.",
                        "Everything public is fair game 🔎",
                        "I find what you forgot to delete",
                        "Metadata never lies 🛰",
                        "Geolocating from shadows",
                    ],
                )
            };
            (first.to_string(), last.to_string(), bio.to_string())
        }
        Kind::Anon => {
            let first = if ru {
                pick(
                    rng,
                    &[
                        "Аноним",
                        "Неизвестный",
                        "Никто",
                        "Гость",
                        "∅",
                        "Тень",
                        "Anonymous",
                    ],
                )
            } else {
                pick(
                    rng,
                    &[
                        "Anonymous",
                        "Unknown",
                        "Nobody",
                        "Guest",
                        "∅",
                        "Shadow",
                        "Null",
                    ],
                )
            };
            let bio = if ru {
                pick(rng, &["", "Меня здесь нет.", "—", "Нет данных.", "404"])
            } else {
                pick(rng, &["", "I am not here.", "—", "No data.", "404"])
            };
            (first.to_string(), String::new(), bio.to_string())
        }
        Kind::Music => {
            let a = pick(
                rng,
                &[
                    "Lo-Fi", "Neon", "Echo", "Vinyl", "Bass", "Synth", "Midnight", "Velvet",
                    "Static", "Nova", "Analog", "Cassette",
                ],
            );
            let b = pick(
                rng,
                &[
                    "Kid", "Tapes", "Beats", "Drive", "Sound", "Club", "Waves", "Room", "",
                ],
            );
            let decor = pick(rng, &["🎧", "🎹", "🎸", "🎶", "📻", "🎛"]);
            let bio = if ru {
                pick(
                    rng,
                    &[
                        "Пишу биты по ночам 🎹",
                        "lo-fi для учёбы и отдыха 🎧",
                        "Сведение, мастеринг, бессонница",
                        "Новый трек уже на подходе 🎶",
                        "Живу на 120 bpm",
                    ],
                )
            } else {
                pick(
                    rng,
                    &[
                        "Making beats at 3am 🎹",
                        "lo-fi beats to study to 🎧",
                        "Mixing, mastering, insomnia",
                        "New track dropping soon 🎶",
                        "Living at 120 bpm",
                    ],
                )
            };
            let first = if b.is_empty() {
                a.to_string()
            } else {
                format!("{a} {b}")
            };
            (first, decor.to_string(), bio.to_string())
        }
        Kind::Art => {
            let a = pick(
                rng,
                &[
                    "Ink", "Moss", "Ochre", "Indigo", "Sienna", "Charcoal", "Pastel", "Umber",
                    "Gouache", "Cobalt", "Sepia", "Mira", "Lina",
                ],
            );
            let b = pick(
                rng,
                &[
                    "Fox", "Studio", "Paints", "Atelier", "Lines", "Sketches", "& Moss", "",
                ],
            );
            let decor = pick(rng, &["🎨", "✏", "🖌", "🖋", "🌿"]);
            let bio = if ru {
                pick(
                    rng,
                    &[
                        "Рисую то, что не могу сказать 🎨",
                        "скетчбук №42 ✏",
                        "Акварель, кофе и прокрастинация",
                        "Открыт(а) для коммишенов 🖌",
                        "Каждый день — новый набросок",
                    ],
                )
            } else {
                pick(
                    rng,
                    &[
                        "Painting what I can't say 🎨",
                        "sketchbook #42 ✏",
                        "Watercolor, coffee, procrastination",
                        "Commissions open 🖌",
                        "A sketch a day",
                    ],
                )
            };
            let first = if b.is_empty() {
                a.to_string()
            } else {
                format!("{a} {b}")
            };
            (first, decor.to_string(), bio.to_string())
        }
    };
    Identity {
        first_name,
        last_name,
        about,
    }
}

// ---------------------------------------------------------------------------------------------
// Pictures from the internet
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Source {
    /// A JSON API; I read the picture URL at the given path.
    Json {
        url: String,
        path: &'static [&'static str],
    },
    /// A URL that answers with the picture itself.
    Direct(String),
    /// Openverse: openly licensed photos and public-domain works found by a search query.
    Openverse { query: String, public_domain: bool },
}

fn dicebear(style: &str, seed: &str) -> Source {
    Source::Direct(format!(
        "https://api.dicebear.com/9.x/{style}/png?seed={seed}&size=512&backgroundColor=b6e3f4,c0aede,d1d4f9,ffd5dc,ffdfbf"
    ))
}

fn openverse(query: &str) -> Source {
    Source::Openverse {
        query: query.to_string(),
        public_domain: false,
    }
}

fn sources(kind: Kind, rng: &mut impl Rng) -> Vec<Source> {
    let seed = random_tail(rng, 10);
    let mut list = match kind {
        Kind::Anime => vec![
            Source::Json {
                url: format!(
                    "https://nekos.best/api/v2/{}",
                    pick(rng, &["neko", "waifu", "kitsune", "husbando"])
                ),
                path: &["results", "0", "url"],
            },
            Source::Json {
                url: format!(
                    "https://nekos.best/api/v2/{}",
                    pick(rng, &["neko", "waifu", "kitsune"])
                ),
                path: &["results", "0", "url"],
            },
            dicebear("adventurer", &seed),
        ],
        Kind::Tiktok => vec![
            dicebear(
                pick(
                    rng,
                    &[
                        "lorelei",
                        "notionists",
                        "avataaars",
                        "micah",
                        "big-smile",
                        "personas",
                    ],
                ),
                &seed,
            ),
            openverse(pick(
                rng,
                &[
                    "neon lights",
                    "pastel aesthetic",
                    "neon city night",
                    "sunset aesthetic",
                    "ring light",
                ],
            )),
            dicebear("fun-emoji", &seed),
        ],
        Kind::Osint => vec![
            openverse(pick(
                rng,
                &[
                    "hacker keyboard",
                    "source code screen",
                    "circuit board",
                    "server room",
                    "radar screen",
                    "world map dark",
                ],
            )),
            dicebear(pick(rng, &["bottts", "bottts-neutral", "identicon"]), &seed),
            Source::Direct(format!(
                "https://robohash.org/{seed}.png?set=set1&size=512x512"
            )),
        ],
        Kind::Anon => vec![
            dicebear(pick(rng, &["identicon", "shapes", "rings", "glass"]), &seed),
            openverse(pick(
                rng,
                &[
                    "fog night",
                    "silhouette dark",
                    "venetian mask",
                    "smoke dark",
                    "foggy forest",
                ],
            )),
            Source::Direct(format!(
                "https://robohash.org/{seed}.png?set=set3&size=512x512"
            )),
        ],
        Kind::Music => vec![
            openverse(pick(
                rng,
                &[
                    "electric guitar",
                    "vinyl record",
                    "synthesizer",
                    "headphones",
                    "concert stage",
                    "piano keys",
                    "turntable",
                ],
            )),
            openverse(pick(rng, &["music studio", "cassette tape", "drum kit"])),
            dicebear("notionists", &seed),
        ],
        Kind::Art => vec![
            Source::Openverse {
                query: pick(
                    rng,
                    &[
                        "oil painting",
                        "landscape painting",
                        "watercolor",
                        "impressionism",
                        "still life painting",
                        "japanese woodblock print",
                    ],
                )
                .to_string(),
                public_domain: true,
            },
            openverse(pick(
                rng,
                &[
                    "paint brushes",
                    "watercolor palette",
                    "sketchbook drawing",
                    "art studio",
                ],
            )),
            dicebear("thumbs", &seed),
        ],
    };
    // I try the themed sources in a random order but keep the generated fallback last.
    let last = list.pop();
    list.shuffle(rng);
    list.extend(last);
    list
}

async fn get(url: &str) -> Result<http::Response> {
    http::send(http::Request {
        method: "GET".into(),
        url: url.to_string(),
        timeout: Some(20),
        limit: Some(IMAGE_LIMIT),
        ..http::Request::default()
    })
    .await?
    .into_success()
}

fn json_path<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for key in path {
        current = match key.parse::<usize>() {
            Ok(index) => current.get(index)?,
            Err(_) => current.get(key)?,
        };
    }
    current.as_str()
}

/// I turn a source into a picture URL and a label that credits where it came from.
async fn resolve(source: &Source, page: u32, pick_index: usize) -> Result<(String, String)> {
    match source {
        Source::Direct(url) => Ok((url.clone(), http::host_of(url))),
        Source::Json { url, path } => {
            let value = get(url).await?.json()?;
            let found = json_path(&value, path)
                .with_context(|| format!("{} returned no picture", http::host_of(url)))?;
            Ok((found.to_string(), http::host_of(url)))
        }
        Source::Openverse {
            query,
            public_domain,
        } => {
            let mut url = format!(
                "https://api.openverse.org/v1/images/?q={}&page_size=20&page={page}&mature=false",
                query.replace(' ', "+")
            );
            if *public_domain {
                url.push_str("&license=cc0,pdm");
            }
            let value = get(&url).await?.json()?;
            let results = value
                .get("results")
                .and_then(|r| r.as_array())
                .cloned()
                .unwrap_or_default();
            let usable = results
                .iter()
                .filter(|r| {
                    let w = r.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                    let h = r.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
                    w >= 300 && h >= 300 && r.get("url").and_then(|v| v.as_str()).is_some()
                })
                .collect::<Vec<_>>();
            let chosen = usable
                .get(pick_index % usable.len().max(1))
                .context("Openverse found no fitting picture")?;
            let text = |key: &str| chosen.get(key).and_then(|v| v.as_str()).unwrap_or_default();
            let mut label = format!("Openverse · {}", text("source"));
            if !text("creator").is_empty() {
                label.push_str(&format!(" · {}", text("creator")));
            }
            if !text("license").is_empty() {
                label.push_str(&format!(
                    " · {} {}",
                    text("license").to_uppercase(),
                    text("license_version")
                ));
            }
            Ok((text("url").to_string(), label.trim().to_string()))
        }
    }
}

/// I crop a picture to a centered square, scale it to 640 px and flatten transparency.
fn square_jpeg(bytes: &[u8]) -> Result<Vec<u8>> {
    let image = image_crate::load_from_memory(bytes).context("not a picture I can read")?;
    let (w, h) = image.dimensions();
    if w < 64 || h < 64 {
        bail!("the picture is too small");
    }
    let side = w.min(h);
    let square = image
        .crop_imm((w - side) / 2, (h - side) / 2, side, side)
        .resize_exact(PHOTO_PX, PHOTO_PX, FilterType::Lanczos3)
        .to_rgba8();
    let mut flat = image_crate::RgbImage::new(PHOTO_PX, PHOTO_PX);
    for (x, y, pixel) in flat.enumerate_pixels_mut() {
        let p = square.get_pixel(x, y);
        let a = f32::from(p[3]) / 255.0;
        let blend = |c: u8| (f32::from(c) * a + 255.0 * (1.0 - a)).round() as u8;
        *pixel = image_crate::Rgb([blend(p[0]), blend(p[1]), blend(p[2])]);
    }
    let mut out = Vec::new();
    image_crate::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 92)
        .encode_image(&DynamicImage::ImageRgb8(flat))
        .context("encoding the picture")?;
    Ok(out)
}

/// I download a fitting picture, trying the next source when one fails.
async fn fetch_picture(kind: Kind) -> Result<(Vec<u8>, String)> {
    let (list, page, index) = {
        let mut rng = rand::thread_rng();
        (
            sources(kind, &mut rng),
            rng.gen_range(1..=4),
            rng.gen_range(0..20),
        )
    };
    let mut errors = Vec::new();
    for source in &list {
        let attempt = async {
            let (url, label) = resolve(source, page, index).await?;
            let response = get(&url).await?;
            let jpeg = square_jpeg(&response.body)?;
            anyhow::Ok((jpeg, label))
        };
        match attempt.await {
            Ok(result) => return Ok(result),
            Err(error) => errors.push(error.to_string()),
        }
    }
    bail!("could not download a picture: {}", errors.join("; "))
}

// ---------------------------------------------------------------------------------------------
// Saved state
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedUser {
    pub id: i64,
    pub access_hash: i64,
}

/// One privacy rule as Telegram returned it, with what I need to send it back.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SavedRule {
    AllowAll,
    AllowContacts,
    AllowCloseFriends,
    AllowPremium,
    AllowBots,
    DisallowAll,
    DisallowContacts,
    DisallowBots,
    AllowUsers { users: Vec<SavedUser> },
    DisallowUsers { users: Vec<SavedUser> },
    AllowChats { chats: Vec<i64> },
    DisallowChats { chats: Vec<i64> },
}

impl SavedRule {
    /// `hash` looks up the access hash of a user Telegram listed next to the rules.
    fn capture(rule: &tl::enums::PrivacyRule, hash: impl Fn(i64) -> i64) -> Self {
        use tl::enums::PrivacyRule as R;
        let saved = |ids: &[i64]| {
            ids.iter()
                .map(|id| SavedUser {
                    id: *id,
                    access_hash: hash(*id),
                })
                .collect()
        };
        match rule {
            R::PrivacyValueAllowAll => SavedRule::AllowAll,
            R::PrivacyValueAllowContacts => SavedRule::AllowContacts,
            R::PrivacyValueAllowCloseFriends => SavedRule::AllowCloseFriends,
            R::PrivacyValueAllowPremium => SavedRule::AllowPremium,
            R::PrivacyValueAllowBots => SavedRule::AllowBots,
            R::PrivacyValueDisallowAll => SavedRule::DisallowAll,
            R::PrivacyValueDisallowContacts => SavedRule::DisallowContacts,
            R::PrivacyValueDisallowBots => SavedRule::DisallowBots,
            R::PrivacyValueAllowUsers(u) => SavedRule::AllowUsers {
                users: saved(&u.users),
            },
            R::PrivacyValueDisallowUsers(u) => SavedRule::DisallowUsers {
                users: saved(&u.users),
            },
            R::PrivacyValueAllowChatParticipants(c) => SavedRule::AllowChats {
                chats: c.chats.clone(),
            },
            R::PrivacyValueDisallowChatParticipants(c) => SavedRule::DisallowChats {
                chats: c.chats.clone(),
            },
        }
    }

    fn input(&self) -> tl::enums::InputPrivacyRule {
        use tl::enums::InputPrivacyRule as I;
        let users = |list: &[SavedUser]| {
            list.iter()
                .map(|u| {
                    tl::enums::InputUser::User(tl::types::InputUser {
                        user_id: u.id,
                        access_hash: u.access_hash,
                    })
                })
                .collect()
        };
        match self {
            SavedRule::AllowAll => I::InputPrivacyValueAllowAll,
            SavedRule::AllowContacts => I::InputPrivacyValueAllowContacts,
            SavedRule::AllowCloseFriends => I::InputPrivacyValueAllowCloseFriends,
            SavedRule::AllowPremium => I::InputPrivacyValueAllowPremium,
            SavedRule::AllowBots => I::InputPrivacyValueAllowBots,
            SavedRule::DisallowAll => I::InputPrivacyValueDisallowAll,
            SavedRule::DisallowContacts => I::InputPrivacyValueDisallowContacts,
            SavedRule::DisallowBots => I::InputPrivacyValueDisallowBots,
            SavedRule::AllowUsers { users: list } => {
                I::InputPrivacyValueAllowUsers(tl::types::InputPrivacyValueAllowUsers {
                    users: users(list),
                })
            }
            SavedRule::DisallowUsers { users: list } => {
                I::InputPrivacyValueDisallowUsers(tl::types::InputPrivacyValueDisallowUsers {
                    users: users(list),
                })
            }
            SavedRule::AllowChats { chats } => I::InputPrivacyValueAllowChatParticipants(
                tl::types::InputPrivacyValueAllowChatParticipants {
                    chats: chats.clone(),
                },
            ),
            SavedRule::DisallowChats { chats } => I::InputPrivacyValueDisallowChatParticipants(
                tl::types::InputPrivacyValueDisallowChatParticipants {
                    chats: chats.clone(),
                },
            ),
        }
    }

    /// I describe a rule for the restore report.
    pub fn describe(&self) -> String {
        match self {
            SavedRule::AllowAll => "everybody".into(),
            SavedRule::AllowContacts => "contacts".into(),
            SavedRule::AllowCloseFriends => "close friends".into(),
            SavedRule::AllowPremium => "premium users".into(),
            SavedRule::AllowBots => "bots allowed".into(),
            SavedRule::DisallowAll => "nobody".into(),
            SavedRule::DisallowContacts => "not contacts".into(),
            SavedRule::DisallowBots => "bots blocked".into(),
            SavedRule::AllowUsers { users } => format!("+{} users", users.len()),
            SavedRule::DisallowUsers { users } => format!("-{} users", users.len()),
            SavedRule::AllowChats { chats } => format!("+{} chats", chats.len()),
            SavedRule::DisallowChats { chats } => format!("-{} chats", chats.len()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedPhoto {
    pub id: i64,
    pub access_hash: i64,
    pub file_reference: Vec<u8>,
    /// A local copy, so I can upload the photo again if Telegram no longer accepts the id.
    #[serde(default)]
    pub file: Option<String>,
}

impl SavedPhoto {
    fn from_photo(photo: &tl::enums::Photo) -> Option<(Self, tl::types::Photo)> {
        let tl::enums::Photo::Photo(photo) = photo else {
            return None;
        };
        Some((
            SavedPhoto {
                id: photo.id,
                access_hash: photo.access_hash,
                file_reference: photo.file_reference.clone(),
                file: None,
            },
            photo.clone(),
        ))
    }

    fn input(&self) -> tl::enums::InputPhoto {
        tl::enums::InputPhoto::Photo(tl::types::InputPhoto {
            id: self.id,
            access_hash: self.access_hash,
            file_reference: self.file_reference.clone(),
        })
    }
}

/// Everything I change, as it was before the first persona.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Original {
    pub first_name: String,
    pub last_name: String,
    pub about: String,
    pub username: Option<String>,
    pub privacy: Vec<SavedRule>,
    pub fallback: Option<SavedPhoto>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersonaState {
    pub kind: Kind,
    pub applied_at: i64,
    pub original: Original,
    pub identity: Option<Identity>,
    pub username: Option<String>,
    /// The public photo I uploaded for the persona.
    pub photo: Option<SavedPhoto>,
    pub image_source: Option<String>,
    /// True while the old username is parked on the reserve channel.
    #[serde(default)]
    pub username_parked: bool,
}

/// A private channel of the account that holds the old username while a persona is active.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Reserve {
    channel_id: i64,
    access_hash: i64,
}

impl Reserve {
    fn input(&self) -> tl::enums::InputChannel {
        tl::enums::InputChannel::Channel(tl::types::InputChannel {
            channel_id: self.channel_id,
            access_hash: self.access_hash,
        })
    }
}

fn state_key(user_id: i64) -> String {
    format!("{STATE_PREFIX}{user_id}")
}

fn reserve_key(user_id: i64) -> String {
    format!("{RESERVE_PREFIX}{user_id}")
}

pub async fn state(db: &Database, user_id: i64) -> Option<PersonaState> {
    serde_json::from_value(db.get(&state_key(user_id)).await).ok()
}

async fn save_state(db: &Database, user_id: i64, state: &PersonaState) -> Result<()> {
    db.set(state_key(user_id), serde_json::to_value(state)?)
        .await
}

async fn reserve(db: &Database, user_id: i64) -> Option<Reserve> {
    serde_json::from_value(db.get(&reserve_key(user_id)).await).ok()
}

fn persona_dir(user_id: i64) -> PathBuf {
    PathBuf::from(DATA_DIR).join(user_id.to_string())
}

// ---------------------------------------------------------------------------------------------
// Telegram helpers
// ---------------------------------------------------------------------------------------------

fn rpc_name(error: &InvocationError) -> Option<&str> {
    match error {
        InvocationError::Rpc(rpc) => Some(rpc.name.as_str()),
        _ => None,
    }
}

/// The largest size of a photo, so I can save a copy of the original public photo.
struct FullPhoto(tl::types::Photo);

impl Downloadable for FullPhoto {
    fn to_raw_input_location(&self) -> Option<tl::enums::InputFileLocation> {
        let largest = self
            .0
            .sizes
            .iter()
            .filter_map(|size| match size {
                tl::enums::PhotoSize::Size(size) => Some((size.r#type.clone(), size.size)),
                tl::enums::PhotoSize::Progressive(size) => {
                    size.sizes.last().map(|bytes| (size.r#type.clone(), *bytes))
                }
                _ => None,
            })
            .max_by_key(|(_, bytes)| *bytes)?;
        Some(tl::enums::InputFileLocation::InputPhotoFileLocation(
            tl::types::InputPhotoFileLocation {
                id: self.0.id,
                access_hash: self.0.access_hash,
                file_reference: self.0.file_reference.clone(),
                thumb_size: largest.0,
            },
        ))
    }
}

async fn capture_original(client: &Client, user_id: i64) -> Result<Original> {
    let tl::enums::users::UserFull::Full(full) = client
        .invoke(&tl::functions::users::GetFullUser {
            id: tl::enums::InputUser::UserSelf,
        })
        .await
        .context("reading the profile")?;
    let tl::enums::UserFull::Full(info) = full.full_user;
    let me = full
        .users
        .iter()
        .find_map(|user| match user {
            tl::enums::User::User(u) if u.is_self || u.id == user_id => Some(u.clone()),
            _ => None,
        })
        .context("Telegram did not return my own profile")?;
    let username = me.username.clone().or_else(|| {
        me.usernames.as_ref().and_then(|list| {
            list.iter().find_map(|name| {
                let tl::enums::Username::Username(name) = name;
                (name.editable && name.active).then(|| name.username.clone())
            })
        })
    });

    let tl::enums::account::PrivacyRules::Rules(privacy) = client
        .invoke(&tl::functions::account::GetPrivacy {
            key: tl::enums::InputPrivacyKey::ProfilePhoto,
        })
        .await
        .context("reading the profile photo privacy")?;
    let rules = privacy
        .rules
        .iter()
        .map(|rule| {
            SavedRule::capture(rule, |id| {
                privacy
                    .users
                    .iter()
                    .find_map(|user| match user {
                        tl::enums::User::User(u) if u.id == id => u.access_hash,
                        _ => None,
                    })
                    .unwrap_or_default()
            })
        })
        .collect();

    let mut fallback = None;
    if let Some((mut saved, photo)) = info
        .fallback_photo
        .as_ref()
        .and_then(SavedPhoto::from_photo)
    {
        let dir = persona_dir(user_id);
        let path = dir.join("original_public_photo.jpg");
        if tokio::fs::create_dir_all(&dir).await.is_ok() {
            let full_photo = FullPhoto(photo);
            let download = client.download_media(&full_photo, &path);
            if let Ok(Ok(())) = tokio::time::timeout(Duration::from_secs(60), download).await {
                saved.file = Some(path.to_string_lossy().to_string());
            }
        }
        fallback = Some(saved);
    }

    Ok(Original {
        first_name: me.first_name.clone().unwrap_or_default(),
        last_name: me.last_name.clone().unwrap_or_default(),
        about: info.about.clone().unwrap_or_default(),
        username,
        privacy: rules,
        fallback,
    })
}

async fn free_username(client: &Client, kind: Kind, identity: &Identity) -> Result<String> {
    let mut last_error = None;
    for _ in 0..USERNAME_ATTEMPTS {
        let candidate = {
            let mut rng = rand::thread_rng();
            username_candidate(kind, identity, &mut rng)
        };
        match client
            .invoke(&tl::functions::account::CheckUsername {
                username: candidate.clone(),
            })
            .await
        {
            Ok(true) => return Ok(candidate),
            Ok(false) => {}
            Err(error) if rpc_name(&error) == Some("USERNAME_INVALID") => {}
            Err(error) => last_error = Some(error),
        }
    }
    match last_error {
        Some(error) => Err(error).context("checking usernames"),
        None => bail!("every generated username was taken"),
    }
}

async fn upload_public_photo(client: &Client, path: &std::path::Path) -> Result<SavedPhoto> {
    let uploaded = client
        .upload_file(path)
        .await
        .context("uploading the photo")?;
    let tl::enums::photos::Photo::Photo(result) = client
        .invoke(&tl::functions::photos::UploadProfilePhoto {
            fallback: true,
            bot: None,
            file: Some(uploaded.raw),
            video: None,
            video_start_ts: None,
            video_emoji_markup: None,
        })
        .await
        .context("setting the public photo")?;
    SavedPhoto::from_photo(&result.photo)
        .map(|(saved, _)| saved)
        .context("Telegram did not return the new photo")
}

async fn delete_photo(client: &Client, photo: &SavedPhoto) {
    let _ = client
        .invoke(&tl::functions::photos::DeletePhotos {
            id: vec![photo.input()],
        })
        .await;
}

async fn set_privacy(client: &Client, rules: Vec<tl::enums::InputPrivacyRule>) -> Result<()> {
    client
        .invoke(&tl::functions::account::SetPrivacy {
            key: tl::enums::InputPrivacyKey::ProfilePhoto,
            rules,
        })
        .await
        .context("changing who can see the profile photo")?;
    Ok(())
}

async fn set_username(client: &Client, username: &str) -> Result<(), InvocationError> {
    client
        .invoke(&tl::functions::account::UpdateUsername {
            username: username.to_string(),
        })
        .await
        .map(drop)
}

/// I answer whether the account may own one more public channel.
async fn can_park_username(client: &Client) -> GuardCheck {
    let probe = format!("fly_probe_{}", random_tail(&mut rand::thread_rng(), 8));
    match client
        .invoke(&tl::functions::channels::CheckUsername {
            channel: tl::enums::InputChannel::Empty,
            username: probe,
        })
        .await
    {
        Ok(_) => GuardCheck::Available,
        Err(error) => match rpc_name(&error) {
            Some("CHANNELS_ADMIN_PUBLIC_TOO_MUCH") => GuardCheck::LimitReached,
            Some(name) if name.starts_with("USERNAME_") => GuardCheck::Available,
            _ => GuardCheck::Unknown(error.to_string()),
        },
    }
}

/// I find or create the private channel that holds the old username.
async fn ensure_reserve(client: &Client, db: &Database, user_id: i64) -> Result<Reserve> {
    if let Some(reserve) = reserve(db, user_id).await {
        let alive = client
            .invoke(&tl::functions::channels::GetChannels {
                id: vec![reserve.input()],
            })
            .await
            .is_ok_and(|chats| match chats {
                tl::enums::messages::Chats::Chats(c) => c
                    .chats
                    .iter()
                    .any(|c| matches!(c, tl::enums::Chat::Channel(_))),
                tl::enums::messages::Chats::Slice(c) => c
                    .chats
                    .iter()
                    .any(|c| matches!(c, tl::enums::Chat::Channel(_))),
            });
        if alive {
            return Ok(reserve);
        }
    }
    let updates = client
        .invoke(&tl::functions::channels::CreateChannel {
            broadcast: true,
            megagroup: false,
            for_import: false,
            forum: false,
            title: RESERVE_TITLE.into(),
            about: "Holds my username while a persona is active. Do not delete.".into(),
            geo_point: None,
            address: None,
            ttl_period: None,
        })
        .await
        .context("creating the reserve channel")?;
    let chats = match &updates {
        tl::enums::Updates::Updates(u) => u.chats.clone(),
        tl::enums::Updates::Combined(u) => u.chats.clone(),
        _ => Vec::new(),
    };
    let channel = chats
        .iter()
        .find_map(|chat| match chat {
            tl::enums::Chat::Channel(c) => c.access_hash.map(|hash| Reserve {
                channel_id: c.id,
                access_hash: hash,
            }),
            _ => None,
        })
        .context("Telegram did not return the new channel")?;
    db.set(reserve_key(user_id), serde_json::to_value(&channel)?)
        .await?;
    // I archive and mute it so it stays out of the way.
    let peer = tl::enums::InputPeer::Channel(tl::types::InputPeerChannel {
        channel_id: channel.channel_id,
        access_hash: channel.access_hash,
    });
    let _ = client
        .invoke(&tl::functions::folders::EditPeerFolders {
            folder_peers: vec![tl::enums::InputFolderPeer::Peer(
                tl::types::InputFolderPeer {
                    peer: peer.clone(),
                    folder_id: 1,
                },
            )],
        })
        .await;
    let _ = client
        .invoke(&tl::functions::account::UpdateNotifySettings {
            peer: tl::enums::InputNotifyPeer::Peer(tl::types::InputNotifyPeer { peer }),
            settings: tl::enums::InputPeerNotifySettings::Settings(
                tl::types::InputPeerNotifySettings {
                    show_previews: None,
                    silent: None,
                    mute_until: Some(i32::MAX),
                    sound: None,
                    stories_muted: None,
                    stories_hide_sender: None,
                    stories_sound: None,
                },
            ),
        })
        .await;
    Ok(channel)
}

async fn set_channel_username(
    client: &Client,
    reserve: &Reserve,
    username: &str,
) -> Result<(), InvocationError> {
    client
        .invoke(&tl::functions::channels::UpdateUsername {
            channel: reserve.input(),
            username: username.to_string(),
        })
        .await
        .map(drop)
}

// ---------------------------------------------------------------------------------------------
// Public operations
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", content = "detail", rename_all = "snake_case")]
pub enum GuardCheck {
    /// The account has no username, or it is already parked.
    NotNeeded,
    Available,
    /// The account owns too many public channels; the old username stays unprotected.
    LimitReached,
    Unknown(String),
}

#[derive(Clone, Debug, Serialize)]
pub struct Check {
    pub active: Option<Kind>,
    pub username: Option<String>,
    pub guard: GuardCheck,
}

/// I look at what applying a persona would do, without changing anything.
pub async fn check(handle: &AccountHandle, db: &Database) -> Result<Check> {
    let current = state(db, handle.user_id).await;
    let username = match &current {
        Some(state) => state.original.username.clone(),
        None => handle.client.get_me().await?.username().map(str::to_string),
    };
    let guard = if username.is_none() || current.as_ref().is_some_and(|s| s.username_parked) {
        GuardCheck::NotNeeded
    } else {
        can_park_username(&handle.client).await
    };
    Ok(Check {
        active: current.map(|s| s.kind),
        username,
        guard,
    })
}

#[derive(Clone, Debug, Serialize)]
pub struct ApplyReport {
    pub kind: Kind,
    pub title: String,
    pub name: String,
    pub username: Option<String>,
    pub about: String,
    pub image_source: Option<String>,
    pub old_username: Option<String>,
    pub username_parked: bool,
    pub first_time: bool,
    pub warnings: Vec<String>,
}

pub async fn apply(handle: &AccountHandle, db: &Database, kind: Kind) -> Result<ApplyReport> {
    let client = &handle.client;
    let user_id = handle.user_id;
    let lang = crate::core_settings::language(db).await;
    let mut warnings = Vec::new();

    let existing = state(db, user_id).await;
    let first_time = existing.is_none();
    let mut current = match existing {
        Some(mut state) => {
            state.kind = kind;
            state
        }
        None => PersonaState {
            kind,
            applied_at: now_secs(),
            original: capture_original(client, user_id).await?,
            identity: None,
            username: None,
            photo: None,
            image_source: None,
            username_parked: false,
        },
    };
    // I save the original before the first change, so a crash in the middle can still be undone.
    save_state(db, user_id, &current).await?;

    let identity = generate_identity(kind, lang, &mut rand::thread_rng());
    let (picture, source) = fetch_picture(kind).await?;
    let dir = persona_dir(user_id);
    tokio::fs::create_dir_all(&dir).await?;
    let picture_path = dir.join("current.jpg");
    tokio::fs::write(&picture_path, &picture).await?;

    set_privacy(
        client,
        vec![tl::enums::InputPrivacyRule::InputPrivacyValueDisallowAll],
    )
    .await?;

    let new_photo = upload_public_photo(client, &picture_path).await?;
    if let Some(previous) = current.photo.replace(new_photo) {
        delete_photo(client, &previous).await;
    }
    current.image_source = Some(source);
    save_state(db, user_id, &current).await?;

    client
        .invoke(&tl::functions::account::UpdateProfile {
            first_name: Some(identity.first_name.clone()),
            last_name: Some(identity.last_name.clone()),
            about: Some(identity.about.clone()),
        })
        .await
        .context("changing the name")?;
    current.identity = Some(identity.clone());
    save_state(db, user_id, &current).await?;

    match free_username(client, kind, &identity).await {
        Ok(new_username) => {
            let old = current.original.username.clone();
            let park = old.is_some() && !current.username_parked;
            // I prepare the channel first, so the old name is free for as short a time as possible.
            let reserve = if park {
                match ensure_reserve(client, db, user_id).await {
                    Ok(reserve) => Some(reserve),
                    Err(error) => {
                        warnings.push(format!("old username not protected: {error:#}"));
                        None
                    }
                }
            } else {
                None
            };
            match set_username(client, &new_username).await {
                Ok(()) => {
                    current.username = Some(new_username);
                    if let (Some(reserve), Some(old)) = (reserve, old.as_deref()) {
                        match set_channel_username(client, &reserve, old).await {
                            Ok(()) => current.username_parked = true,
                            Err(error) => warnings.push(format!(
                                "old username not protected: {}",
                                match rpc_name(&error) {
                                    Some("CHANNELS_ADMIN_PUBLIC_TOO_MUCH") =>
                                        "too many public channels".to_string(),
                                    _ => error.to_string(),
                                }
                            )),
                        }
                    }
                }
                Err(error) => warnings.push(format!("username not changed: {error}")),
            }
        }
        Err(error) => warnings.push(format!("username not changed: {error:#}")),
    }
    save_state(db, user_id, &current).await?;

    Ok(ApplyReport {
        kind,
        title: kind.title(lang).to_string(),
        name: format!("{} {}", identity.first_name, identity.last_name)
            .trim()
            .to_string(),
        username: current.username.clone(),
        about: identity.about,
        image_source: current.image_source.clone(),
        old_username: current.original.username.clone(),
        username_parked: current.username_parked,
        first_time,
        warnings,
    })
}

#[derive(Clone, Debug, Serialize)]
pub struct RestoreReport {
    pub name: String,
    pub username: Option<String>,
    pub privacy: Vec<String>,
    pub public_photo_restored: bool,
    pub errors: Vec<String>,
}

pub async fn restore(handle: &AccountHandle, db: &Database) -> Result<RestoreReport> {
    let client = &handle.client;
    let user_id = handle.user_id;
    let Some(mut current) = state(db, user_id).await else {
        bail!("no persona is active on this account");
    };
    let original = current.original.clone();
    let mut errors = Vec::new();

    // Username: I free the parked name first, then take it back.
    let target = original.username.clone().unwrap_or_default();
    let reserve = reserve(db, user_id).await;
    if current.username_parked {
        if let Some(reserve) = &reserve {
            match set_channel_username(client, reserve, "").await {
                Ok(()) => current.username_parked = false,
                Err(error) if rpc_name(&error) == Some("USERNAME_NOT_MODIFIED") => {
                    current.username_parked = false
                }
                Err(error) => errors.push(format!("reserve channel: {error}")),
            }
        }
    }
    match set_username(client, &target).await {
        Ok(()) => current.username = None,
        Err(error) if rpc_name(&error) == Some("USERNAME_NOT_MODIFIED") => current.username = None,
        Err(error) => errors.push(format!("username: {error}")),
    }
    save_state(db, user_id, &current).await?;

    if let Err(error) = client
        .invoke(&tl::functions::account::UpdateProfile {
            first_name: Some(original.first_name.clone()),
            last_name: Some(original.last_name.clone()),
            about: Some(original.about.clone()),
        })
        .await
    {
        errors.push(format!("name: {error}"));
    }

    // Public photo: the old one if there was one, otherwise none.
    let mut public_photo_restored = false;
    match &original.fallback {
        Some(old) => {
            let by_id = client
                .invoke(&tl::functions::photos::UpdateProfilePhoto {
                    fallback: true,
                    bot: None,
                    id: old.input(),
                })
                .await;
            if by_id.is_ok() {
                public_photo_restored = true;
            } else if let Some(file) = old
                .file
                .as_deref()
                .filter(|f| std::path::Path::new(f).exists())
            {
                match upload_public_photo(client, std::path::Path::new(file)).await {
                    Ok(_) => public_photo_restored = true,
                    Err(error) => errors.push(format!("public photo: {error:#}")),
                }
            } else if let Err(error) = by_id {
                errors.push(format!("public photo: {error}"));
            }
        }
        None => {
            if let Err(error) = client
                .invoke(&tl::functions::photos::UpdateProfilePhoto {
                    fallback: true,
                    bot: None,
                    id: tl::enums::InputPhoto::Empty,
                })
                .await
            {
                if rpc_name(&error) != Some("PHOTO_ID_INVALID") {
                    errors.push(format!("public photo: {error}"));
                }
            }
            public_photo_restored = true;
        }
    }
    if let Some(photo) = current.photo.take() {
        delete_photo(client, &photo).await;
    }

    let rules = original.privacy.iter().map(SavedRule::input).collect();
    if let Err(error) = set_privacy(client, rules).await {
        errors.push(format!("privacy: {error:#}"));
    }

    if errors.is_empty() {
        db.remove(&state_key(user_id)).await?;
        let _ = tokio::fs::remove_dir_all(persona_dir(user_id)).await;
    } else {
        save_state(db, user_id, &current).await?;
    }

    Ok(RestoreReport {
        name: format!("{} {}", original.first_name, original.last_name)
            .trim()
            .to_string(),
        username: original.username,
        privacy: original.privacy.iter().map(SavedRule::describe).collect(),
        public_photo_restored,
        errors,
    })
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub kind: Kind,
    pub title: String,
    pub since: i64,
    pub name: Option<String>,
    pub username: Option<String>,
    pub original_name: String,
    pub original_username: Option<String>,
    pub username_parked: bool,
}

pub async fn status(db: &Database, user_id: i64) -> Option<Status> {
    let lang = crate::core_settings::language(db).await;
    let current = state(db, user_id).await?;
    Some(Status {
        kind: current.kind,
        title: current.kind.title(lang).to_string(),
        since: current.applied_at,
        name: current.identity.as_ref().map(|i| {
            format!("{} {}", i.first_name, i.last_name)
                .trim()
                .to_string()
        }),
        username: current.username.clone(),
        original_name: format!(
            "{} {}",
            current.original.first_name, current.original.last_name
        )
        .trim()
        .to_string(),
        original_username: current.original.username.clone(),
        username_parked: current.username_parked,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_follow_telegram_rules() {
        assert_eq!(
            clean_username("__Neko__Chan__").as_deref(),
            Some("neko_chan")
        );
        assert_eq!(clean_username("42abc_def").as_deref(), Some("abc_def"));
        assert_eq!(clean_username("ab"), None);
        assert!(clean_username(&"a".repeat(50)).is_some_and(|u| u.len() == 32));
        let mut rng = rand::thread_rng();
        for kind in Kind::ALL {
            for lang in [Lang::En, Lang::Ru] {
                for _ in 0..200 {
                    let identity = generate_identity(kind, lang, &mut rng);
                    assert!(!identity.first_name.trim().is_empty(), "{kind:?}");
                    assert!(identity.about.chars().count() <= 70, "{}", identity.about);
                    let name = username_candidate(kind, &identity, &mut rng);
                    assert!((5..=32).contains(&name.len()), "{name}");
                    assert!(name.starts_with(|c: char| c.is_ascii_lowercase()), "{name}");
                    assert!(!name.ends_with('_') && !name.contains("__"), "{name}");
                    assert!(name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'));
                }
            }
        }
    }

    #[test]
    fn persona_names_parse_in_both_languages() {
        assert_eq!(Kind::parse("Анимешник"), Some(Kind::Anime));
        assert_eq!(Kind::parse("tiktok"), Some(Kind::Tiktok));
        assert_eq!(Kind::parse("осинтер"), Some(Kind::Osint));
        assert_eq!(Kind::parse("аноним"), Some(Kind::Anon));
        assert_eq!(Kind::parse("музыкант"), Some(Kind::Music));
        assert_eq!(Kind::parse("художник"), Some(Kind::Art));
        assert!(Kind::parse("random").is_some());
        assert_eq!(Kind::parse("pirate"), None);
    }

    #[test]
    fn privacy_rules_round_trip_with_exceptions() {
        let rules = [
            tl::enums::PrivacyRule::PrivacyValueAllowContacts,
            tl::enums::PrivacyRule::PrivacyValueAllowUsers(tl::types::PrivacyValueAllowUsers {
                users: vec![7],
            }),
            tl::enums::PrivacyRule::PrivacyValueDisallowChatParticipants(
                tl::types::PrivacyValueDisallowChatParticipants { chats: vec![5] },
            ),
        ];
        let saved = rules
            .iter()
            .map(|r| SavedRule::capture(r, |id| if id == 7 { 99 } else { 0 }))
            .collect::<Vec<_>>();
        let json = serde_json::to_value(&saved).unwrap();
        let back: Vec<SavedRule> = serde_json::from_value(json).unwrap();
        let input = back.iter().map(SavedRule::input).collect::<Vec<_>>();
        assert!(matches!(
            input[0],
            tl::enums::InputPrivacyRule::InputPrivacyValueAllowContacts
        ));
        match &input[1] {
            tl::enums::InputPrivacyRule::InputPrivacyValueAllowUsers(u) => match &u.users[0] {
                tl::enums::InputUser::User(user) => {
                    assert_eq!((user.user_id, user.access_hash), (7, 99))
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
        match &input[2] {
            tl::enums::InputPrivacyRule::InputPrivacyValueDisallowChatParticipants(c) => {
                assert_eq!(c.chats, vec![5])
            }
            other => panic!("{other:?}"),
        }
    }

    /// `cargo test persona_pictures_live -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "uses the network"]
    async fn persona_pictures_live() {
        for kind in Kind::ALL {
            for _ in 0..2 {
                let (jpeg, label) = fetch_picture(kind).await.unwrap();
                let image = image_crate::load_from_memory(&jpeg).unwrap();
                assert_eq!(image.dimensions(), (PHOTO_PX, PHOTO_PX));
                println!("{kind:?}: {} KB from {label}", jpeg.len() / 1024);
            }
        }
    }

    #[test]
    fn pictures_become_square_jpegs() {
        let mut png = Vec::new();
        image_crate::RgbaImage::from_pixel(300, 200, image_crate::Rgba([10, 20, 30, 0]))
            .write_to(
                &mut std::io::Cursor::new(&mut png),
                image_crate::ImageOutputFormat::Png,
            )
            .unwrap();
        let jpeg = square_jpeg(&png).unwrap();
        let back = image_crate::load_from_memory(&jpeg).unwrap();
        assert_eq!(back.dimensions(), (PHOTO_PX, PHOTO_PX));
        // Transparent pixels become white.
        assert!(back.to_rgb8().get_pixel(5, 5)[0] > 240);
    }
}
