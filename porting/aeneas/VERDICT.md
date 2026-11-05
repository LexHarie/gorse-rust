# Aeneas spike verdict (P1-3)

## Result

- **Loop (`sum_u32`, a `u32` loop over a slice): translates and builds.** This is the spike's pass condition, and it is met at the pins below.
- **f32 (`dot_f32`): not translated at this pin.** Aeneas rejects the crate at a float literal. The f32 question is therefore open. The spike did not test f32 arithmetic on its own.

## Evidence

CI run [42020587265](https://github.com/LexHarie/gorse-rust/actions/runs/42020587265), job 127222053231, on head `a8249912`. The job succeeded, and its log ends with:

```
loop (sum_u32): translated and built
f32 (dot_f32): not translated or not built (status 1); see the log above
```

The f32 log shows the failure. Charon translated `aeneas_spike_f32` to LLBC. Aeneas then reported:

```
[Error] Improperly typed constant value
Source: 'src/lib.rs', lines 9:23-9:26
```

Lines 9:23 to 9:26 of `porting/aeneas/spike-f32/src/lib.rs` are the literal `0.0` in `let mut acc: f32 = 0.0;`. Aeneas did not translate the body of `dot_f32`, and it wrote a partial Lean file.

## Pins

| Component | Revision | Archive sha256 (`git archive HEAD`) |
|---|---|---|
| Aeneas | `c2d234386474622b6aeda5add0e95ff995f49210` | `4196510beda034998fbe685819f099262beb19fe9f89224b906d08d12c921026` |
| Charon (the one Aeneas pins) | `a58d159210b3a36fedeaba4437bda896a8b12f0d` | `b3f5abb9e0d432c3d7944f3f495a344b344eb4bf7cdd8f3269ef6433f9cade07` |

Charon uses the nightly named in its own `rust-toolchain`, `nightly-2026-09-17`. The Aeneas Lean library uses Lean `v4.35.0`, with Mathlib `v4.35.0` through its own `lake-manifest.json`. OCaml is 5.3.0.

## What this does not show

- **Float support in general.** The one failing construct is a float constant, and the spike did not separate it from f32 arithmetic. A follow-up crate without float literals, for example one that takes the starting accumulator as a parameter, would test arithmetic on its own.
- **Lean output for `dot_f32`.** It was not generated in full, so nothing is known about its Lean.
- **The staged workflow.** The run that produced this verdict used the single-step script. The three-stage workflow (`charon`, `aeneas`, `lean`) is on the same branch and has not run yet.

## Consequence for the plan

Floating-point code cannot be translated at the current Aeneas pin, at least for code with float constants. The plan's f32 items need a decision. Either they stay out of the verified scope, or the pin moves to a newer Aeneas where the spike is re-run. That decision belongs to the coordinator, not to this spike.
