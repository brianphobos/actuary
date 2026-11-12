# Job spec: slugify

Write `slugify(text)` in a file that exports it with `module.exports = { slugify }`.

- Lowercase ASCII letters and digits are kept.
- Accented Latin letters become their base letter (é → e).
- Every run of other characters becomes a single hyphen.
- No leading or trailing hyphens.
- Output is at most 60 characters, cut at a hyphen boundary when possible.

Acceptance: all 24 cases in `cases.json` pass.
