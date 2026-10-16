# Export and recovery

The panel has **Chat exports** and **Session recovery** pages. Both require a panel
session and the same request protection as account management.

## Exports

Choose accounts, optionally select individual chats, then choose a method and format.
The Takeout method opens a Telegram Takeout session for history requests and closes
it after collection. If Telegram returns `TAKEOUT_INIT_DELAY`, the task shows the
server's delay and the time when it can be retried. JSON and HTML are rendered locally;
Telegram's API returns structured protocol objects, not a ready-made archive.
Attachments are downloaded through the regular authenticated media API.

Custom exports use ordinary history requests without Takeout's initial waiting
period. Telegram's normal rate limits still apply. They support JSON, HTML, both,
or PDF for one selected chat. PDF requires Chromium on the machine running the
userbot. Set `FLY_PDF_BROWSER` to its executable if it is not named `chromium`.

Options include:

- Included and excluded chat IDs, date range, message limit per chat.
- Photos, videos, stickers, GIFs, video notes, voice messages, files and audio.
- Per-file and total archive size limits. Skipped attachments retain a placeholder
  and the reason they were skipped. A task fails if the final document exceeds the
  total limit. Text metadata has an additional 128 MiB collection limit.
- Profile and phone number, profile photos, contacts, device/session information,
  local/chat settings, Telegram global privacy options and notification preferences.
  Exported device information contains no MTProto keys,
  recovery private keys, bot tokens, or Telegram 2FA passwords.
- Custom filters for phrases, sender IDs and whole messages containing selected
  media types. These filters also apply to archived deleted messages.

Deleted messages are merged by account, chat and message ID. The archive includes
their deletion timestamps. Messages that were never captured by anti-delete cannot
be recovered from Telegram. Existing archived attachments follow the same media
filters and size limits as live attachments.

Global default rules and account overrides are stored in `export.rules`. Scheduled
custom exports are stored in `export.schedules`. A schedule compares the selected
message content, edits and archived deletions before downloading attachments and
writing another archive. Schedules persist across restarts. Progress, cancellation,
errors and Takeout delays appear in Tasks.

Completed exports are private files under `data/exports/<uuid>/`. The panel can open
the documents or download a `.tar.gz` containing the documents and media. Exported
HTML escapes message content and works offline. Generated exports contain the data
you selected, so deleting an account from recovery does not erase copies already
exported or downloaded.

The HTML reader includes avatar galleries, search, reply links, edit markers, a
deleted-only filter and a reading bookmark stored in that browser. Custom emoji
keep their Telegram document IDs and use their text fallback in offline HTML/PDF;
animated custom emoji assets are not embedded.

## Recovery vault

Create a separate vault password in Session recovery. The vault at
`keys/recovery.enc` uses Argon2id and XChaCha20-Poly1305. Software ES256 passkeys are
registered through `account.initPasskeyRegistration` and `account.registerPasskey`.
The credential is persisted in encrypted form before registration to avoid losing
it if the application stops during the request.

Enable registration globally, including future accounts, or override it for an
account. New sessions are queued for at least 24 hours; additional server delays
are shown per account. Automatic registration requires an unlocked vault. With
automatic registration enabled, the unlocked vault remains in process memory until
you lock it or stop the application. Otherwise it locks after 15 minutes of
inactivity. Unlock it again after a restart; its password is not stored on disk.

Restoration creates a new session using `auth.initPasskeyLogin` and
`auth.finishPasskeyLogin` in the account's data center. If Telegram requests 2FA,
the panel opens a password dialog. That password is used for SRP authentication and
is not saved to the database, vault, files or browser storage. The account ID is
checked before the restored session is attached to the userbot.

Deleting access removes the local credential, session files and the account's
anti-delete records. If the account is connected, its server passkey is revoked
and that session is logged out first. If it is disconnected, revoke the server
passkey from another Telegram device as well. This action does not delete the
Telegram account itself. Regular application backups exclude the recovery vault;
keep a private copy of the encrypted vault if you need a separate recovery backup.

## Music and custom emoji

Run `music-worker` in the same data directory as the main application. It creates
`keys/music.token` for authenticated local control. Install `ffmpeg` and `yt-dlp`.
Start a Telegram broadcast with an external encoder and give the account stream
management access. `.play` and `.vplay` obtain the Telegram RTMP destination and
send media to it. Queues are separate for each account and chat; completion advances
the queue and failed FFmpeg processes are reported. Ordinary non-RTMP voice chats
still require a WebRTC/tgcalls transport and are not handled by this worker.

Modules can send Telegram premium/custom emoji with
`ui.custom_emoji("document_id", "⭐")` or markdown
`[⭐](tg://emoji?id=123456789)`. Document IDs must be passed as decimal strings when
they cannot be represented exactly by a caller's number type. Telegram decides
whether the account is permitted to send a particular custom emoji. JSON exports
preserve custom emoji document IDs and UTF-16 entity offsets.

## Development checks

```sh
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
python3 tests/panel_smoke.py
```

The browser test uses local fixtures. Live Takeout, passkey registration and login,
Telegram 2FA, and RTMP streaming require a test account and cannot be validated by
the fixture tests alone.

Current workspace validation: Rust formatting and JavaScript syntax were checked.
Rust compilation and tests could not run because the dependency cache is empty and
crates.io is unavailable. Chromium was blocked by the local sandbox; the panel test
was stopped at the user's request. Telegram API,
2FA recovery, PDF rendering and RTMP streaming still need integration validation.
