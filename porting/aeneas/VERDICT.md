# Aeneas spike verdict (P1-3)

## Result

- **Loop (`sum_u32`, a `u32` loop over a slice): translates and builds.** This is the spike's pass condition, and it is met at the pins below. It is met in the staged workflow as well.
- **f32 at this pin: rejected, both ways we tried.** Aeneas rejects the float literal `0.0` in `dot_f32`, and it rejects the f32 multiplication `a[i] * b[i]` in the probe, which has no float literal.
- **Decision (President's ruling):** floating-point items stay out of verified scope at pin `c2d2343`. They go to tier II under the G9 fallback (amendment 2). There is no pin bump during P1.

## Evidence

Loop and f32, single-step script: CI run [42020587265](https://github.com/LexHarie/gorse-rust/actions/runs/42020587265), job 127222053231, head `a8249912`. The loop passed; the `dot_f32` log shows:

```
[Error] Improperly typed constant value
Source: 'src/lib.rs', lines 9:23-9:26
```

Lines 9:23 to 9:26 of `porting/aeneas/spike-f32/src/lib.rs` are the literal `0.0` in `let mut acc: f32 = 0.0;`.

Staged workflow, the three stages: CI run [42024400971](https://github.com/LexHarie/gorse-rust/actions/runs/42024400971), job 127234859915, head `1cca4d4a`. Every stage passed, and the lean stage printed `loop (sum_u32): translated and built`. The log shows the probe result:

```
[Error] Invalid inputs for binop
Source: 'src/lib.rs', lines 13:15-13:26
```

In `porting/aeneas/spike-f32-probe/src/lib.rs`, lines 13:15 to 13:26 are `a[i] * b[i]`. The probe takes the accumulator as a parameter and has no float literal, so the rejection is of the f32 multiplication itself. The stage still passed, because the f32 crates are reported, not required.

## Pins

| Component | Revision | Archive sha256 (`git archive HEAD`) |
|---|---|---|
| Aeneas | `c2d234386474622b6aeda5add0e95ff995f49210` | `4196510beda034998fbe685819f099262beb19fe9f89224b906d08d12c921026` |
| Charon (the one Aeneas pins) | `a58d159210b3a36fedeaba4437bda896a8b12f0d` | `b3f5abb9e0d432c3d7944f3f495a344b344eb4bf7cdd8f3269ef6433f9cade07` |

Charon uses the nightly named in its own `rust-toolchain`, `nightly-2026-09-17`. The Aeneas Lean library uses Lean `v4.35.0`, with Mathlib `v4.35.0` through its own `lake-manifest.json`. OCaml is 5.3.0.

## What this does not show

- **Whether a newer Aeneas handles f32.** The pin was not moved, by the President's ruling. A newer pin would be a separate decision, and the spike would run again on it.
- **Lean output for the f32 crates.** Both produced only partial Lean files, so nothing is known about their Lean.

## Consequence for the plan

Floating-point code is outside verified scope at the current Aeneas pin. Under the G9 fallback (amendment 2), those items go to tier II. The loop, which is the spike's pass condition, is verified through Aeneas and the Lean build.
