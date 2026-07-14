# Score Duration Rejection Implementation Report

## Problem

A legitimate Roady leaderboard submission with score **1614** was rejected as
`invalid_duration` / `round_duration_ms out of range`. The old Worker limit was
120,000 ms. That confused the remaining-clock ceiling with elapsed active play:
coins and Time pickups can repeatedly add time, so a valid round can last more
than 120 seconds while its remaining clock stays capped.

## Implemented contract

- `round_duration_ms` is accepted from **0 through 1,800,000 ms**, inclusive.
- 1,800,000 ms (30 minutes) is a generous elapsed-play anti-abuse ceiling.
- The Worker still requires a JSON number that is an integer, non-negative, and
  no greater than `Number.MAX_SAFE_INTEGER`; the 30-minute bound is then applied.
- Fractional, negative, unsafe/overflowed, string, null, missing, and otherwise
  malformed duration values remain rejected with `invalid_duration`.
- `time_left_ms` retains its unrelated 120,000 ms ceiling.
- Score caps, total/component equality, combo plausibility, session, Turnstile,
  replay, signature, body-size, and all other validation remain unchanged.

The contract constants are:

- Rust client: `src/leaderboard.rs` — `MAX_ROUND_DURATION_MS: u64 = 1_800_000`
- Worker: `leaderboard/src/validation.ts` — `MAX_ROUND_DURATION_MS = 1_800_000`

A Worker test reads the Rust source constant and asserts equality, preventing
silent client/Worker drift.

## Signing and payload safety

The canonical HMAC field order is unchanged. Duration continues to be emitted
as canonical base-10 text with one LF separator and no trailing LF. Tests verify
that the same extended duration produces identical canonical bytes and changing
it by 1 ms changes the bytes. Worker validation occurs before canonical byte
construction, preserving exact safe-integer serialization.

No payload or request-size limit was relaxed.

## Visible client errors

Submission failures now identify their category and action:

- `VALIDATION [code]: message; retry unchanged will fail` for payload/validation responses
- `SERVER [code]: ... retry ...` for retryable session/service responses
- `TURNSTILE [...]` for browser or Worker verification failures
- `NETWORK [session|score]: ... retry` for known transport failures; unstructured HTTP failures use `NETWORK/SERVER [http]` rather than being mislabeled as validation

This makes a stable server validation code visible while distinguishing it from
network and Turnstile failures. The existing explicit retry UI remains intact.

## Tests added or expanded

Worker/unit and route coverage now includes:

- reported score 1614 with `round_duration_ms = 161_400` accepted
- exact 1,800,000 ms accepted
- 1,800,001 ms rejected
- fractional, negative, unsafe overflow, numeric string, null, and missing
  duration rejected
- unrelated `time_left_ms` upper bound retained
- canonical signature bytes stable for the same extended duration
- Rust client and Worker maximum constants aligned
- end-to-end Worker route accepts and stores score 1614 with >120-second duration
- client error formatting exposes validation codes and retry classification

## Verification constraint

Per task instruction, no shell, build, test, npm, git, web, or spawned process
was executed. Changes were reviewed statically only.
