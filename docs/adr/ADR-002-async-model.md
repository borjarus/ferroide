# ADR-002: Async model — GPUI executor for UI and logic, Tokio only in the background

- **Status:** Accepted
- **Date:** 2026-11-06
- **Stage:** 0

## Context

GPUI ships its own executors: a foreground executor bound to the UI thread
and a background thread pool, reached through `cx.spawn` and
`cx.background_spawn`. Several libraries FerroIDE will need — `sqlx`,
`reqwest`, `bollard` — require a Tokio runtime. Running two general-purpose
runtimes side by side risks futures being polled on the wrong one, and any
blocking call on the UI thread breaks the frame budget (plan §5.4).

## Decision

1. **UI and logic use GPUI's executors.** Views spawn with `cx.spawn`;
   CPU-bound or I/O-bound work goes through `cx.background_spawn`.
2. **Service contracts are executor-agnostic.** Traits in `ferro_services`
   return `futures::future::BoxFuture<'static, Result<T>>` that are `Send`,
   and publish events over `async_channel` receivers. Implementations may use
   `smol`/`async-io` primitives (timers, processes), which work under any
   executor. The Stage 1 mocks are an example.
3. **Tokio lives in one dedicated background runtime** (1–2 worker threads),
   created lazily the first time a Tokio-only library is used (Stage 6 for
   server downloads, Stage 12 for databases and Docker). Its futures are
   bridged back to GPUI through channels or `tokio::task::JoinHandle`
   awaited from a background task — never polled on the UI thread.
4. **Cancellation:** every long-running request holds a cancellation token or
   is a dropped `Task`; dropping a GPUI `Task` cancels it. A new edit cancels
   stale completion/hover/semantic-token requests (Stage 6).

## Consequences

- Logic crates depend on `futures`, `async-channel` and `smol`, not on GPUI,
  so they are testable with `smol::block_on` (see `ferro_services` tests).
- The UI thread never awaits anything slower than a channel receive.
- One more runtime's threads exist once Tokio-based features are used; they
  are idle otherwise.

## Alternatives considered

- **Tokio everywhere.** GPUI's foreground work must run on its executor, so
  every UI interaction would cross runtimes; rejected.
- **No Tokio at all.** Would rule out `sqlx`, `reqwest` and `bollard` or
  force forks; rejected for Stages 6 and 12.
