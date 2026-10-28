> ℹ️ **INTERNAL DOCUMENT — NOT PUBLISHED.** `.gitignore` keeps this file out of git
> (`*.md` except `README.md`/`LICENSE.md`, plus the whole `docs/` dir). Verify with
> `git check-ignore -v <this file>`; a silent `git status` about it is expected.
> **Language policy:** English is mandatory for technical docs here. Legacy
> Ukrainian/Russian sections are translated in a dedicated pass — never mix languages
> inside a section, and never translate quoted commit subjects, log lines or CLI output
> (they are evidence, they stay byte-exact). See `AGENTS.md` §4 for the full policy.
> **Numbers** (test counts, versions) must come from a real run, not from arithmetic.

# 🔑 KeyPolicy — the key classification contract

> Status: **IMPLEMENTED (2026-10-01).** This document specifies the API that
> closed the "my hotkey kills my own clicker" bug class. Every line number below was
> read out of the tree at commit `379970b`; nothing here is reconstructed from memory.
>
> ⚠️ **§3 changed during implementation.** The contract is no longer "call
> `exempt_all` *after* `rebuild`" — it is **two separate bitmaps**, so `rebuild`
> *cannot* clear the binding shield regardless of call order. A unit test
> (`a_user_cannot_unexempt_a_bound_hotkey`) failed against the single-bitmap
> version, which is why the design changed. Read §3 before touching the type.

---

## 1. Why this document exists

The user bound the toggle hotkey to **`J`**. The clicker never started — anywhere, in
any application, with any CPS. The presets worked (`Mouse4` / `Mouse5` fired fine), so
the hook was alive and the channel was healthy. The cause is a self-inflicted kill:

```
user presses J
  └─ platform/windows/mod.rs:559   key-down, classified Fresh
      ├─ mod.rs:632  is_text_keypress_vk(0x4A /* J */)  →  true
      │              ← because J is NOT in typing.rs:65 GAMEPLAY_LETTERS
      │                (that list is exactly: A D E F Q R S W)
      └─ mod.rs:634  scheduler.set_active(false, …)      ← CLICKER STOPPED
      └─ mod.rs:661  guard.try_arm_toggle(…)              ← start also refused
                     └─ typing.rs:272  hotkeys_locked() == true  →  veto
```

**One keypress, two contradicting decisions.** The kill-switch fires first and stops
the clicker; the toggle gate then refuses to start it because the lockout that same
keypress just armed is still open. The net effect is a hotkey that is guaranteed never
to start the clicker, on the exact key the user pressed to start it.

`J` is not a forgotten letter in a list. It is the end of a class: **the classifier has
no knowledge of which keys the user bound to hotkeys.** `is_ignored_key_vk` hardcodes
eight letters; `HotkeyBindings` (mod.rs:1071-1074) already holds every bound combo in
memory, and nothing ever connects the two.

## 2. Why the obvious fix is the wrong fix

Extending the hardcoded list to `A D E F Q R S W C X Z V B G H J K L N M` makes the
current symptom disappear and the next one appear. A user who binds `T` (open chat),
`Y` (quick-swap), `U` or `O` gets the identical deadlock on a build that "fixed" `J`.

A static list cannot solve a dynamic question. The set of keys that must not arm the
lockout is *derived from the user's configuration*, so the answer must be derived from
the user's configuration too. The list becomes a **seed**, not the mechanism.

## 3. Two layers, and why one list cannot hold both

| Layer | What it is | Owner | Removable? |
|---|---|---|---|
| **A — binding shield** | every key the user bound to any hotkey | system, automatic | **NO, never** |
| **B — gameplay keys** | letters the user plays with but never bound | **user, via Settings UI** | yes |

Layer A is not negotiable and is not stored in the config: it is recomputed from
`HotkeyBindings` every time the hook re-parses. Layer B is the user-facing list
(see `docs/IGNORED_KEYS_FEATURE.md`).

**The invariant is enforced by the TYPE, not by call order.** `KeyPolicy` holds
**two independent bitmaps**:

```rust
pub struct KeyPolicy {
    words_seed:  [AtomicU64; 4],   // layer B: hardcoded seed + the user's list
    words_bound: [AtomicU64; 4],   // layer A: the binding shield, set-bit-only
}
```

* `rebuild()` (layer B) rewrites `words_seed` **only**.
* `exempt()` / `exempt_all()` (layer A) set bits in `words_bound` **only**.
* `is_exempt()` ORs the two words.

So "a config save can never un-exempt a bound hotkey" is **structural**: there is
no code path that clears `words_bound` at all. The first implementation used one
bitmap and relied on the hook re-applying `exempt_all` after every `rebuild`; the
test `a_user_cannot_unexempt_a_bound_hotkey` **failed against it**, because
between `rebuild` and the next re-parse there was a real window in which the
toggle key looked like ordinary typing — exactly the bug this module exists to
kill, just narrower. Splitting the bitmaps removed the window rather than
documenting it.

The hook still calls `exempt_all` after each re-parse (that is how layer A learns
about bindings), but it is now a **refresh, not a load-bearing ordering rule**.

---

## 4. API surface

New module: **`src-tauri/src/guard/key_policy.rs`** (`pub mod key_policy` in
`guard/mod.rs`). `GAMEPLAY_LETTERS` and `is_ignored_key_vk` **moved here**;
`typing.rs` keeps `is_text_keypress_vk` only for tests and pure-function callers,
and `is_typable_vk` remains its single definition (`key_policy` imports it).

```rust
/// TWO bitmaps — see §3. Never collapse these into one.
pub struct KeyPolicy {
    words_seed:  [AtomicU64; 4],   // layer B
    words_bound: [AtomicU64; 4],   // layer A, set-bit-only
}

impl KeyPolicy {
    /// Seed from the hardcoded defaults (layer B, no user list yet).
    pub fn new() -> Self;

    /// Rewrite layer B only. The ONLY caller is `ClickScheduler::set_config`.
    /// Cannot clear layer A — it writes a different bitmap.
    pub fn rebuild(&self, user_labels: &[String]);

    /// Layer A, automatic. Set-bit-only: there is no public way to REMOVE one.
    pub fn exempt(&self, vk: u16);
    pub fn exempt_all(&self, vks: impl Iterator<Item = u16>);

    /// Read side. Hot path — no allocation, no syscalls.
    pub fn is_exempt(&self, vk: u16) -> bool;          // layer B OR layer A
    pub fn is_text_keypress(&self, vk: u16) -> bool;   // vk != 0 && !is_exempt
    pub fn is_typable(&self, vk: u16) -> bool;         // exempt ⇒ never typable

    /// Diagnostics for Settings → Input diagnostics. Not on any hot path.
    pub fn report(&self) -> KeyPolicyReport;
}

/// Pure classification, exported for tests and the Settings UI.
pub fn is_protected_key(vk: u16) -> bool;
pub fn is_typable_vk(vk: u16) -> bool;              // re-export of typing.rs
pub fn default_ignored_labels() -> Vec<String>;
pub fn key_label(vk: u16) -> String;
pub fn vk_from_label(label: &str) -> Option<u16>;
pub fn check_ignore_label(label: &str, already_in_list: bool) -> IgnoreKeyReport;
```

`is_exempt` is **two** relaxed loads and two bit tests, not one — the cost of the
structural guarantee described in §3.

`KeyPolicyReport` (serialised to the page):

```rust
#[derive(serde::Serialize)]
pub struct KeyPolicyReport {
    pub exempt_total: u32,          // layer B ∪ layer A
    pub gameplay_total: u32,        // size of the hardcoded seed
    pub non_seed_keys: Vec<String>, // user list + bound keys -> the 🔒 chips
}
```

`IgnoreKeyReport` is defined next to it and is the reply of `validate_ignore_key`
(`ok`, `reason`, `protected`, `already_bound`).

### The two questions, kept apart

| Question | Nature | Function |
|---|---|---|
| "Could typing produce this key?" | property of the key | `is_typable_vk` (typing.rs:140) |
| "Should this press arm the lockout?" | **policy + bindings** | `KeyPolicy::is_text_keypress` |

They are currently the same question in one function, which is why the bug exists.
`typing.rs:140` stays a pure static predicate; the dynamic decision moves to `KeyPolicy`.

## 5. Zero-Jitter contract

`KeyPolicy` is read from the `WH_KEYBOARD_LL` callback path, so the rules in
`AGENTS.md` §2.17 apply unchanged: **no lock, no allocation, no file I/O, no logging,
no `emit` on the read side**, and a single writer (the hook loop, between event
batches — never a callback).

The full contract is in **Appendix A** at the end of this file.

## 6. Integration points

Four call sites change. Each one is a **replacement**, never an addition — the free
function must end up with zero production callers, or the two sources of truth drift.

### `platform/windows/mod.rs`

```rust
impl HotkeyBindings {
    /// Every trigger AND every required key in every group.
    pub fn policy_keys(&self) -> impl Iterator<Item = u16> + '_ {
        self.all_groups().iter().flat_map(|g| g.iter())
            .flat_map(|c| std::iter::once(c.trigger).chain(c.required.iter().copied()))
    }
}
```

`required` matters as much as `trigger`: a chord like `Ctrl+J` holds `Ctrl`, and
holding a modifier must never arm the lockout either.

In the re-parse block (mod.rs, after `HotkeyBindings::from_snapshot`):

```rust
apply_binding_shield(&scheduler, &bindings);
```

**And the same call at STARTUP, right after the initial bindings are built.**
This is not optional. The re-parse block is guarded by
`current_version != seen_version`, and `seen_version` is *seeded with the current
version* — so at boot the block never runs. Calling the shield only from there
leaves layer A **empty until the first config save**, and every bare-letter toggle
(`R`, `J`, `T`) is dead in exactly that window: the key is classified as typing,
the kill-switch stops the clicker, and the guard vetoes the start. Saving any
setting bumps `hotkeys_version` and the bind revives, which is why the symptom
looked intermittent — it survived every config change and came back on each
restart.

Three classification sites moved from the free function to the policy:

| Site | Was | Now |
|---|---|---|
| kill-switch | `guard::is_text_keypress_vk(event.vk)` | `scheduler.key_policy().is_text_keypress(event.vk)` |
| guard arming | `guard::is_text_keypress_vk(event.vk)` | `scheduler.key_policy().is_text_keypress(event.vk)` |
| toggle gate | `guard::is_typable_vk(combo.trigger)` | `scheduler.key_policy().is_typable(combo.trigger)` |

### `scheduler.rs`

Two fields, two accessors:

```rust
key_policy: Arc<crate::guard::KeyPolicy>,
/// Kept so `get_config()` can hand the list back — a real round-trip exists
/// (tests, and the preset path in `windows/mod.rs`), and returning an empty
/// list would erase the user's keys on the next save.
typing_ignore_keys: Arc<Mutex<Vec<String>>>,
pub fn key_policy(&self) -> &KeyPolicy { &self.key_policy }
```

Seeded in `new()`, rebuilt in `set_config`.

### `config.rs` / `config_manager.rs`

`Config` gains `typing_ignore_keys: Vec<String>`, carried by `From<AppConfig>`.
The persisted field is `UiSettings::typing_ignore_keys` with `#[serde(default)]`, so
**no migration entry is needed** — an absent field deserialises to the empty list,
which means "use the hardcoded seed".

`GuardSnapshot` (config_manager.rs:437, the `scope.guards` preset group) is
deliberately **not** extended: the ignored-key list is machine-specific, and a preset
that carried it would silently rewrite one game's keys from another game's profile.

## 7. Tripwires

Tests 1-13 live in `key_policy.rs`; tests 14-19 in `src-tauri/tests/test_assets.rs`.

| # | Test | What it traps |
|---|---|---|
| 1 | `a_bound_key_can_never_arm_the_lockout_that_gates_it` | **The primary one.** `exempt(0x4A)` ⇒ `!is_text_keypress(0x4A)`. Direct encoding of this bug. |
| 2 | `the_seed_is_in_force_after_construction` | WASD / Space / arrows stay ignored; `J` and `T` are ordinary text until bound. |
| 3 | `rebuild_applies_the_user_list` | User labels are applied and a previous list is replaced, never merged. |
| 4 | `an_unknown_label_is_skipped_not_fatal` | A hand-edited `config.json` cannot take the input layer down. |
| 5 | `a_user_cannot_unexempt_a_bound_hotkey` | `rebuild(["C","X"])` after `exempt(0x4A)` ⇒ `J` **stays** exempt. **This is the test that failed against the single-bitmap design and forced §3.** |
| 6 | `a_user_cannot_unexempt_a_protected_key` | Same for a key the user never sees listed. |
| 7 | `an_exempt_key_is_never_typable` | An ignored key also skips the toggle-confirmation window. |
| 8 | `the_static_typable_predicate_is_unchanged` | `F6` / mouse buttons stay non-typable when unbound. |
| 9 | `codes_above_the_bitmap_are_safe` | Out-of-range vk is ignored, never an index panic. |
| 10 | `exempting_twice_is_idempotent` | Re-application on every re-parse is harmless. |
| 11 | `labels_round_trip` | Every label the UI shows parses back. **Caught two real bugs**: `key_label` printed `vk + 1` while `vk_from_label` subtracted 1, so `Mouse1` round-tripped to `0x00` and the seed test failed with `"Mouse1" must be a seed key`. The label and the parser now agree on `Mouse{vk}`. |
| 12 | `the_default_list_is_parseable_and_unique` | The seed has no duplicate or unparseable label. **Caught a real bug**: `key_label` mapped both `0x5B` and `0x5C` to `"Win"`, so the default list carried a duplicate and the UI could not tell the two apart. All left/right modifier variants are now labelled distinctly (`LShift`/`RShift`, `LCtrl`/`RCtrl`, `LAlt`/`RAlt`, `WinLeft`/`WinRight`). |
| 13 | `the_pre_add_check_explains_its_refusal` | `validate_ignore_key` names the reason instead of failing silently. |
| 14 | `the_hook_uses_the_policy_not_the_free_function` | Wiring: the hook calls `key_policy()`, and `guard::is_text_keypress_vk(event.vk)` appears **zero** times. |
| 15 | `the_binding_shield_is_applied_on_every_reparse` | **The startup call is the load-bearing one.** Counts the `apply_binding_shield(&scheduler, &bindings)` call sites and requires **two**: one after the initial bindings, one inside the version-change block. The first version of this test compared two `find()` offsets and passed while the shield ran ONLY on re-parse — `find()` matched the *startup* `from_snapshot` and the single `exempt_all` sat later in the file, so the assertion was satisfied by the wrong call. That is how "all tests green, hotkey still dead" shipped. Also checks all nine binding groups **plus required modifiers**. |
| 16 | `the_key_policy_takes_no_lock_on_the_hot_path` | Source scan: no `Mutex`/`RwLock`/`String`/`Vec` in the struct, and **two named bitmaps** (`words_seed` + `words_bound`). |
| 17 | `ignored_keys_roundtrip_through_config` | The list is persisted, rebuilt in `set_config`, returned by `get_config`, and the three commands are registered. |
| 18 | `our_own_writes_are_never_reported_as_external` | The watcher grace is **derived** from the poll interval, and no call site marks a config write by hand. |
| 19 | `test_key_policy_doc_exists_and_names_the_invariant` | This file cannot be deleted silently. |

Three of these (#5, #9, #11/#12) **failed on real runs and found real bugs in the
implementation**, which is the reason to keep them.

## 8. Rules for anyone touching this

* **Never call `is_text_keypress_vk` from production code again.** It is a static
  predicate that cannot know the bindings. Test 14 enforces the two live sites;
  grep for the symbol before adding a third.
* **Never merge `words_seed` and `words_bound` into one bitmap.** That was the
  original design and test #5 proved it leaves a window where a bound toggle key
  looks like ordinary typing. Two bitmaps, one per layer, forever. Test 16
  enforces the field names.
* **Never add a `clear`/un-exempt for layer A.** `exempt` is set-bit-only on
  purpose: a bound hotkey is a correctness invariant, not a preference.
* **Never add a lock to "make the layers safer".** The bitmaps are
  independently owned and the read path is two atomic loads; a lock here would
  violate §2.17 for no benefit.
* **Never make layer A configurable.** It is not a preference. If it looks
  configurable, the design has drifted.
* **Do not widen the hardcoded seed to "fix" a report.** Widen it only with keys
  that are plausibly game keys *and* are not already covered by a binding. Test 1
  is the real fix; the seed is a convenience for unbound keys.

## 9. Related

* `docs/IGNORED_KEYS_FEATURE.md` — the user-facing layer B (UI, storage, commands).
* `docs/FIELD_BUG_REPORT.md` — the six measured field bugs, including this one.
* `AGENTS.md` §2 — where the invariant is summarised for future agents.
* `tech.md` — factor **F22** in the failure table.

## 10. Chords and Mouse Buttons in Layer A

* **Multi-key chords (`*+1`, `Ctrl+Alt+M`):** Layer A iterates over `policy_keys()`
  which extracts BOTH the trigger and every required modifier (`group_keys`).
  For instance, `*+1` marks both `0x6A`/`0x38` (multiply / shift+8) and `0x31` ('1')
  in `words_bound`. Consequently, holding `*` can never arm the typing lockout.
* **Mouse Buttons (`Mouse4`, `Mouse5`):** While mouse buttons are routed via `WH_MOUSE_LL`,
  their virtual-key representations (`0x04`, `0x05`, `0x06`) are strictly non-typable
  (`is_typable_vk == false`). They bypass toggle confirmation delays completely.

---

## Appendix A. Zero-Jitter contract (full text)

This type is read from the `WH_KEYBOARD_LL` callback path. The rules are the ones in
`AGENTS.md` §2.17, not new ones:

* **No lock, ever.** Not `Mutex`, not `RwLock`, not a `OnceLock` read on the hot path.
* **No allocation.** No `Vec`, no `String`, no iterator that yields owned values, on the
  read side. `is_exempt` is four `Ordering::Relaxed` loads and a bit test.
* **No file I/O, no logging, no `emit`.** Diagnostics go to `hotkey_diag_push`
  (the in-memory ring, mod.rs:926), which the hook *loop* drains.
* **Single writer.** `rebuild` and `exempt_all` are called only from the hook loop
  between event batches, never from inside a callback. There is therefore no
  writer/reader race to defend against — the invariant is structural, not a lock.

`exempt_all` takes an `impl Iterator`, so the borrow of `bindings` does not allocate;
the loop body is a bit-set.
