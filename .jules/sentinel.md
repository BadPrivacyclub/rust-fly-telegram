## 2024-05-24 - [Argument Injection in yt-dlp]
**Vulnerability:** Command argument injection in `src/music_worker.rs` when calling `yt-dlp`. User-supplied `query` was interpolated directly into the argument list without isolating it as a positional parameter. If a payload started with `-` (e.g. `-o`), it would be parsed as an option.
**Learning:** Always use `--` to signify the end of command line options before passing user-controlled strings as positional arguments to external commands executed via `std::process::Command`.
**Prevention:** Append `--` to `args` right before the untrusted input parameter (`.args(["... options ...", "--", &query])`).
