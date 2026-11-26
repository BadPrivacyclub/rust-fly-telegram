//! Limit on how long Lua code may run without giving control back to Rust.
//!
//! The deadline covers one stretch of execution: one synchronous call, or one poll of an async
//! call. Every `await` inside a module ends the stretch, so a module that waits for the network
//! is never stopped, while a loop that never yields is.

use std::cell::Cell;
use std::future::Future;
use std::time::{Duration, Instant};

use mlua::{HookTriggers, Lua, VmState};

/// Longest stretch for a sandboxed module.
pub const SANDBOXED: Duration = Duration::from_secs(2);
/// Longest stretch for a trusted module.
pub const TRUSTED: Duration = Duration::from_secs(15);
/// Instructions between two deadline checks.
const CHECK_EVERY: u32 = 1000;

thread_local! {
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

pub fn limit_for(trusted: bool) -> Duration {
    if trusted {
        TRUSTED
    } else {
        SANDBOXED
    }
}

/// Error-catching functions re-raise an error once the deadline has passed, so the error from
/// the hook cannot be swallowed by a loop around `pcall`.
const RETHROW_AFTER_DEADLINE: &str = r#"
local expired, raw_pcall, raw_xpcall, raw_resume = ...
local function pass(ok, ...)
    if not ok and expired() then
        error((...), 0)
    end
    return ok, ...
end
pcall = function(...) return pass(raw_pcall(...)) end
xpcall = function(...) return pass(raw_xpcall(...)) end
coroutine.resume = function(...) return pass(raw_resume(...)) end
"#;

fn expired() -> bool {
    DEADLINE
        .get()
        .is_some_and(|deadline| Instant::now() >= deadline)
}

/// Installs the hook that stops code running past the deadline of the current thread. Code
/// started without [`run`] or [`limited`] has no deadline. Must run before any module or
/// sandbox environment takes `pcall`, `xpcall`, or `coroutine` from the globals.
pub fn install(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();
    let coroutine = globals.get::<mlua::Table>("coroutine")?;
    lua.load(RETHROW_AFTER_DEADLINE)
        .set_name("budget")
        .call::<()>((
            lua.create_function(|_, ()| Ok(expired()))?,
            globals.get::<mlua::Function>("pcall")?,
            globals.get::<mlua::Function>("xpcall")?,
            coroutine.get::<mlua::Function>("resume")?,
        ))?;
    lua.set_global_hook(
        HookTriggers::new().every_nth_instruction(CHECK_EVERY),
        |_, _| {
            if expired() {
                Err(mlua::Error::runtime(
                    "the module ran too long without a pause and was stopped",
                ))
            } else {
                Ok(VmState::Continue)
            }
        },
    )
}

/// Runs `f` with a deadline `limit` from now. A nested call never extends an outer deadline.
pub fn run<T>(limit: Duration, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Instant>);
    impl Drop for Restore {
        fn drop(&mut self) {
            DEADLINE.set(self.0);
        }
    }

    let new = Instant::now() + limit;
    let previous = DEADLINE.get();
    DEADLINE.set(Some(previous.map_or(new, |outer| outer.min(new))));
    let _restore = Restore(previous);
    f()
}

/// Wraps an async Lua call so that every poll gets its own deadline.
#[cfg(test)]
pub fn limited<F: Future>(limit: Duration, future: F) -> impl Future<Output = F::Output> {
    let mut future = Box::pin(future);
    std::future::poll_fn(move |cx| run(limit, || future.as_mut().poll(cx)))
}

/// Like [`limited`], and also returns the time spent inside polls: the time the call kept a
/// thread busy, without the time it waited.
pub fn measured<F: Future>(
    limit: Duration,
    future: F,
) -> impl Future<Output = (F::Output, Duration)> {
    let mut future = Box::pin(future);
    let mut spent = Duration::ZERO;
    std::future::poll_fn(move |cx| {
        let started = Instant::now();
        let poll = run(limit, || future.as_mut().poll(cx));
        spent += started.elapsed();
        poll.map(|output| (output, spent))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua() -> Lua {
        let lua = Lua::new();
        install(&lua).unwrap();
        lua
    }

    #[test]
    fn an_endless_loop_is_stopped() {
        let lua = lua();
        let started = Instant::now();
        let result = run(Duration::from_millis(100), || {
            lua.load("while true do end").exec()
        });
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn catching_the_error_does_not_keep_an_endless_loop_alive() {
        let lua = lua();
        for source in [
            "while true do pcall(function() while true do end end) end",
            "while true do xpcall(function() while true do end end, function(e) return e end) end",
            "while true do coroutine.resume(coroutine.create(function() while true do end end)) end",
        ] {
            let result = run(Duration::from_millis(100), || lua.load(source).exec());
            assert!(result.is_err(), "{source}");
        }
    }

    #[test]
    fn pcall_still_catches_ordinary_errors() {
        let lua = lua();
        let (ok, message): (bool, String) = run(Duration::from_secs(5), || {
            lua.load(r#"local ok, e = pcall(error, "boom", 0) return ok, e"#)
                .eval()
        })
        .unwrap();
        assert!(!ok);
        assert_eq!(message, "boom");
    }

    #[test]
    fn code_without_a_deadline_runs_to_the_end() {
        let lua = lua();
        let sum: i64 = lua
            .load("local s = 0 for i = 1, 1000000 do s = s + i end return s")
            .eval()
            .unwrap();
        assert_eq!(sum, 500_000_500_000);
    }

    #[tokio::test]
    async fn an_await_starts_a_new_stretch() {
        let lua = lua();
        let sleep = lua
            .create_async_function(|_, ms: u64| async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                Ok(())
            })
            .unwrap();
        lua.globals().set("sleep", sleep).unwrap();
        let call = lua
            .load("return function() for _ = 1, 3 do sleep(80) end return true end")
            .eval::<mlua::Function>()
            .unwrap();
        let done: bool = limited(Duration::from_millis(100), call.call_async(()))
            .await
            .unwrap();
        assert!(done);

        let endless = lua
            .load("return function() sleep(1) while true do end end")
            .eval::<mlua::Function>()
            .unwrap();
        let result = limited(Duration::from_millis(100), endless.call_async::<()>(())).await;
        assert!(result.is_err());
    }
}
