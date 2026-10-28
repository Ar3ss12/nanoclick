> ℹ️ **INTERNAL DOCUMENT — NOT PUBLISHED.** `.gitignore` keeps this file out of git
> (`*.md` except `README.md`/`LICENSE.md`, plus the whole `docs/` dir). Verify with
> `git check-ignore -v <this file>`; a silent `git status` about it is expected.
> **Language policy:** English is mandatory for technical docs here. Legacy
> Ukrainian/Russian sections are translated in a dedicated pass — never mix languages
> inside a section, and never translate quoted commit subjects, log lines or CLI output
> (they are evidence, they stay byte-exact). See `AGENTS.md` §4 for the full policy.
> **Numbers** (test counts, versions) must come from a real run, not from arithmetic.

# 🎮 Ignored Keys — user-configurable Typing Guard keys

> Status: **IMPLEMENTED (2026-10-01).** Specifies the user-facing half of
> `KeyPolicy` (layer B). Read `docs/KEY_POLICY.md` first — this document assumes the
> two-layer model and does not repeat it.

---

## 1. The feature in one paragraph

Today the keys that never arm the Typing Guard are eight hardcoded letters
(`typing.rs:65`: `A D E F Q R S W`) plus a static list of digits, function keys,
arrows and modifiers. A player whose game uses `C`, `X`, `Z`, `V`, `B`, `J` or `L`
gets the clicker silently stopped by their own movement keys. The user asked for the
list to become **editable in the interface**: add the keys your game uses, remove the
ones it does not, see what is currently in force.

Layer B is that list. It is a seed, never a guarantee — keys bound to hotkeys are
always ignored regardless (layer A, `docs/KEY_POLICY.md` §3).

## 2. Storage

One new field, no migration:

```rust
// src-tauri/src/config_manager.rs — UiSettings
/// Keys treated as gameplay: they never arm the Typing Guard. Empty = the
/// hardcoded seed. Keys bound to a hotkey are exempt regardless of this list.
#[serde(default)]
pub typing_ignore_keys: Vec<String>,
```

Empty is a legal, meaningful value — it means "the hardcoded seed only". Absent is the
same thing (`#[serde(default)]`), so an existing `config.json` needs no repair pass and
no entry in `defaults/mod.rs`.

Values are canonical key labels (`"C"`, `"F5"`, `"Mouse4"`), stored as typed by the
user and normalised by the backend (`trim`; `key_label` is used for display).
Persisting the **label** rather than the numeric code keeps the file readable and
survives a layout change; the bit-set is rebuilt from labels on every `set_config`.

**A round-trip hazard, found while wiring it.** `ClickScheduler::get_config()` →
`set_config()` really does happen (unit tests, and the preset path in
`windows/mod.rs`). Returning an empty list from `get_config` would therefore erase
the user's keys on the very next save, so the scheduler keeps its own
`typing_ignore_keys: Arc<Mutex<Vec<String>>>` and hands it back. Tripwire #17.

**Not** added to `GuardSnapshot` (config_manager.rs:437). A preset that carried this
list would carry one machine's game bindings into another machine's profile, which is
the same class of bug as F21 (a preset replacing the library).

## 3. UI — a new section in Settings

Placement: inside the BEHAVIOR card, directly under `#guardStatus`
(`index.html:603`), so it sits with the Typing Guard checkbox and the freeze-window
input it modifies. No new tab, no new card — the feature belongs to the card that
already owns the guard.

```html
<div class="settings-row" style="margin-top:8px;padding-top:8px;border-top:1px solid rgba(255,255,255,0.06);">
  <span class="settings-label" data-i18n="settings_typing_ignore_label">
    Ignored keys (never treated as typing)
  </span>
</div>
<div class="settings-hint" data-i18n="settings_typing_ignore_hint">
  Keys you bind to a hotkey are always ignored automatically.
</div>
<div id="typingKeyEntryRow" style="display:flex;gap:8px;margin-top:6px;">
  <button id="typingKeyCaptureBtn" class="preset-modal-btn cancel" style="flex:1;">
    <span data-i18n="settings_typing_ignore_capture">Press a key…</span>
  </button>
  <input id="typingKeyEntry" class="preset-input" placeholder="C, X, Z, V, B, J…"
         style="flex:1;" maxlength="64">
  <button id="typingKeyAddBtn" class="preset-modal-btn save"
          data-i18n="settings_typing_ignore_add">Add</button>
</div>
<div id="typingKeyList" class="app-filter-list"></div>
```

### Two ways to add a key

1. **Press it.** `typingKeyCaptureBtn` arms a one-shot `keydown` capture. This is a
   deliberate copy of the hotkey recorder's shape (`main.js` `setupHotkeyRecorder`)
   with three differences: modifiers are **not** collected (a modifier is never a
   gameplay letter), there is no TTL timer (one key, one add), and **Escape
   cancels** — without that last one the capture would keep swallowing every key on
   the page until one was finally pressed. The listener is removed on every exit
   path (`finish()`), so it cannot leak.
2. **Type it.** `typingKeyEntry` accepts a comma-separated list, for users who know
   the letters already. Normalised, de-duplicated, capped at a sane length.

### Rendering the chips

`settings_guard.js` already owns `_list` / `addEntry` / `removeEntry` / `render` for the
app filter (settings_guard.js:565-586) and already delegates deletes through a
container listener because rows are re-created on every render (settings_guard.js:140-147).
**The ignored-key list copies that pattern exactly.** It is the same list widget with
different data — no new widget, no new delegation scheme.

Chips show two states:

| State | Rendered as | Why |
|---|---|---|
| User-listed | label + ✕ delete button | removable |
| Bound to a hotkey | label + 🔒 badge, **no** ✕ | layer A, not configurable |

A key the user just bound to a hotkey stays in the list but becomes locked. That is
honest: it is in force, and the user cannot remove it. `validate_ignore_key` (§4) is what
tells the UI which state applies.

`hydrate()` reads `ui.typing_ignore_keys`; `collect()` writes it back. Both are the
existing methods (settings_guard.js:518 / :542) — no new bridge.


## 4. Backend commands

Three, all read-only or list-scoped. None of them is on a hot path.

```rust
/// Diagnostics for Settings → Input diagnostics.
#[tauri::command]
fn get_key_policy(state: State<'_, AppState>) -> KeyPolicyReport;

/// Instant check before a key is added to the list.
#[tauri::command]
fn validate_ignore_key(label: String, state: State<'_, AppState>) -> IgnoreKeyReport;

/// The canonical seed, for a "Reset to defaults" action.
#[tauri::command]
fn get_default_ignored_keys() -> Vec<String>;

#[derive(serde::Serialize)]
pub struct IgnoreKeyReport {
    pub label: String,
    pub vk: u16,
    pub ok: bool,             // may this be added to layer B?
    pub reason: Option<&'static str>,   // "protected" | "unknown_label" | "duplicate"
    pub protected: bool,      // in the hardcoded seed
    pub already_bound: bool,  // layer A — always ignored regardless
}
```

`validate_ignore_key` is what makes the UI honest instead of permissive. Typing `Ctrl`
returns `protected: true` and the button refuses. Typing `J` when `J` is bound to the
toggle returns `already_bound: true` and the UI says "already ignored automatically —
no need to add it", which teaches the two-layer model at the moment it matters.

This follows the rule in `AGENTS.md` §2.11: *a hotkey that "does nothing" is a log line,
not a theory* — the same principle applied to a key that "does nothing because it is
already exempt".

## 5. Settings → Input diagnostics

The existing diagnostics card (where `dump_input_diagnostics` lives, main.js:2769) gains
a `key-policy-report` block rendering `get_key_policy()`:

```
In force: 34 keys ignored (20 gameplay · 14 bound to hotkeys)
Your list: C, X, Z, V, B, J
Locked:   J (toggle), Mouse4 (preset PvP), Mouse5 (preset pve)
```

The point is diagnosability. When a user reports "the clicker still stops", the answer
is now visible without reading `%TEMP%\nanoclick_web.log`: the report says whether the
key is in the seed, in the user list, or bound — and therefore why.

## 6. Localisation

Roughly 14 new keys × 2 locales (`en`, `ua`) in `src/locales/`. The i18n
symmetry tripwire (`test_notice_i18n_symmetry`) fails on any asymmetry, so both
files are edited in the same pass.

Existing keys that must be **reworded**, because they now promise something the code
delivers differently:

| Key | Current text | Change |
|---|---|---|
| `settings_typing_guard` | "Stop clicking while typing, never switch on (WASD ignored)" | drop the hardcoded `(WASD ignored)` — the ignored set is now user-visible and editable |
| `settings_guard_status_idle` | "Stops clicking for {ms} ms after you type (WASD and hotbar ignored)" | same; point at the new list instead |
| `settings_typing_freeze_label` | "Hotkey lockout after typing (ms)" | unchanged, but sits next to the new section |

Wording matters here: a label that hardcodes "WASD" while the list is editable is a
small lie that will read as a bug report later.

## 7. Files touched

| File | Change |
|---|---|
| `src-tauri/src/config_manager.rs` | `UiSettings::typing_ignore_keys` |
| `src-tauri/src/config.rs` | `Config::typing_ignore_keys` + `From<AppConfig>` |
| `src-tauri/src/guard/key_policy.rs` | new — see `docs/KEY_POLICY.md` |
| `src-tauri/src/scheduler.rs` | field + `rebuild` on `set_config` |
| `src-tauri/src/commands/tauri_commands.rs` | 3 commands |
| `src-tauri/src/lib.rs` | register the 3 commands |
| `src/index.html` | the new section |
| `src/settings_guard.js` | list state, capture, render, hydrate/collect |
| `src/main.js` | nothing beyond calling the commands |
| `src/locales/{en,ua}.json` | ~14 keys each + 3 reworded |

## 8. Tests

The policy-level tests are in `docs/KEY_POLICY.md` §7. The feature-level ones:

| Test | What it traps |
|---|---|
| `ignored_keys_roundtrip_through_config` | The list survives hydrate → save → restart. |
| `a_protected_key_cannot_be_added_by_the_user` | `validate_ignore_key("Ctrl")` ⇒ `ok == false`. |
| `the_ignored_key_section_exists_in_settings` | Wiring: the ids above are in `index.html` and wired in `settings_guard.js`. |
| `ignored_keys_survive_a_deep_sleep_page_rebuild` | A page rebuilt after deep sleep re-renders the list from the backend, not from a stale module copy. |

The last one matters because `settings_guard.js` holds list state in the module
(`_list`) exactly like the app filter does — a rebuilt page must re-hydrate, or the
user's keys vanish from the UI while still being in force.
