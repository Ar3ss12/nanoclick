# ⚡ NanoClick v4.0 — Інтуїтивний та Потужний Помічник Автоматизації

**NanoClick** — це сучасний настільний додаток для автоматизації натискань миші та клавіатури з повноцінним **macro recorder**, **visual editor** та **advanced control-flow** (loops, conditions, variables). Побудований на Tauri 2.x + Rust backend + vanilla JS/HTML/CSS frontend.

![NanoClick Icon](./src/NanoClick.png)

> **Збірка (v4.0):** `cargo build --release` → `target/release/NanoClick.exe` (**~9.94 MB**)
> **Стек:** Tauri 2.11 + Rust backend + vanilla JS/HTML/CSS frontend (no bundler)
> **Тести:** **57/57 passed** (`cargo test --lib --release`)

---

## 🎯 Що нового у v4.0

| Версія | Фіча | Статус |
|---|---|---|
| **v3.2** | 🔴 Macro Recorder + Action Engine (14 Action variants) | ✅ Готово |
| **v3.2** | Smart Normalizer (5 фаз) — auto-pair clicks, dedupe moves, insert waits | ✅ Готово |
| **v3.3** | ✏️ Visual Editor — drag-reorder, context menu, Re-record | ✅ Готово |
| **v3.3** | ⚡ Optimize button (Subtle / Balanced / Aggressive) | ✅ Готово |
| **v4.0** | 🔁 Repeat blocks (`Repeat { count, inner }`) | ✅ Готово |
| **v4.0** | ❓ Conditional If/Else (`Condition::VarEq`, `VarLt`, `VarGt`, `PixelEquals`) | ✅ Готово |
| **v4.0** | 📞 Call nested macros (`Action::Call { macro_id }`) | ✅ Готово |
| **v4.0** | 💾 Variables (`SetVar`, `GetVar`) + `ExecutionContext` | ✅ Готово |
| **v4.0** | Variable Inspector UI | 🚧 В плані |

---

## 🌟 Основні Можливості

### 🎯 Гнучкі Режими Кліків
- **Single / Double Click:** Класичні поодинокі або подвійні натискання.
- **Hold Mode (Утримання):** Режим утримання кнопки із заданою тривалістю та паузами.
- **Human Jitter:** Рандомізація інтервалів (±0–30%) **та** просторовий jitter (±0–30 px).
- **Position Picker:** Закріпити клікер за конкретною точкою екрана або за курсором.

### 🛡️ Безпека та Зручність
- **Work Mode:** Автоматичне блокування гарячих клавіш при роботі з іншими програмами.
- **Auto-Pause on Navigation:** Автоматичне призупинення клікера при переході у налаштування.
- **Windows Autostart & Tray:** Запуск разом із системою та згортання у трей.

### 🎨 Дизайн та Теми
6 стильних тем: `Dark Cyberpunk`, `Neon Grass`, `Dark Slate`, `Midnight Blue`, `Dracula Crimson`, `Amethyst Purple`. Акцентні свотчі для індивідуального налаштування.

---

## ⌨️ Гарячі Клавіші

| Дія | За замовчуванням |
|---|---|
| 🚀 Старт / Стоп автоклікера | `R` / `K` або `F6` |
| ⚡ Перемикання Autoclicker ↔ Work Mode | `Ctrl+Alt+M` |
| 🛑 Екстрене вимкнення | `Escape` |
| ➕ Збільшити швидкість (+1 CPS) | `Ctrl+=` |
| ➖ Зменшити швидкість (-1 CPS) | `Ctrl+-` |
| 🎯 Захопити координати точки екрана | `Ctrl+P` |
| 🔴 Старт / Стоп запису макросу | `Ctrl+Shift+R` |

> **Технічна нотатка:** Listener використовує Windows `WH_KEYBOARD_LL` і event-driven канал натискань, тому в стані спокою не виконує циклічний `GetAsyncKeyState` polling.

---

## ⏱️ Таймери (всі в Rust)

Усі таймери виконуються в **Rust worker thread**, синхронно з click-loop — без розбіжностей при перезавантаженні UI:

- **Start Delay (сек):** зворотний відлік перед першим кліком (1–300 с).
- **Stop Duration (хв):** автоматична зупинка через N хвилин від старту.
- **Stop Time (HH:MM):** автоматична зупинка о заданій годині (тільки сьогодні).

---

## 🧠 v4.0 — Advanced Automation

### Два шляхи створення макросу:

```
            CREATE MACRO
                 │
        ┌────────┴────────┐
        ↓                 ↓
    🔴 Record          ＋ Build
        │                 │
  реальні дії        ручні блоки
        │                 │
        └────────┬────────┘
                 ↓
            Action List
              (Macro)
```

### 🔁 Repeat
```js
Action::Repeat {
    count: 5,
    inner: vec![
        Action::MouseClick { button: MouseButton::Left, count: 1 },
        Action::Wait { ms: 100 },
        Action::MouseClick { button: MouseButton::Right, count: 1 },
        Action::Wait { ms: 100 },
    ]
}
```

### ❓ Conditional
```js
Action::If {
    condition: Condition::PixelEquals {
        x: 500, y: 300,
        color: [255, 0, 0, 255],
        tolerance: 10,
    },
    then_branch: vec![Action::MouseClick { ... }],
    else_branch: Some(vec![Action::Wait { ms: 1000 }]),
}
```

### 📞 Call + Variables
```js
Action::SetVar { name: "loops", value: 0 }
Action::GetVar { name: "loops", into: last_value_register }
Action::Call { macro_id: "sub_routine" }
```

Підтримуються `Condition::True`, `VarEq { name, value }`, `VarLt`, `VarGt`, `PixelEquals { x, y, color, tolerance }`. Variables зберігаються в `ExecutionContext` (HashMap), call stack обмежений `MAX_CALL_DEPTH = 16`.

**Детальна архітектура:** [`docs/MACRO_ARCHITECTURE.md`](./docs/MACRO_ARCHITECTURE.md) · [`docs/v4_CONTROL_FLOW.md`](./docs/v4_CONTROL_FLOW.md)

---

## 🎬 v3.2/v3.3 — Macro Recorder + Visual Editor

> *Don't program the automation. Perform it once. Then refine it.*

**Workflow:**
1. **`Ctrl+Shift+R`** (або `🔴 Record new macro` в UI).
2. Виконуєш дії вручну.
3. **`Ctrl+Shift+R`** знову — запис зупиняється.
4. **Visual Editor:** послідовність блоків, drag-reorder, inline-edit значень.
5. **`⚡ Optimize`** — прибирає зайві moves, об'єднує Down/Up.
6. Зберігаєш → макрос у вкладці **Automation**.

**Smart Normalizer** — обов'язковий за замовчуванням:
| Інше | NanoClick |
|---|---|
| `MouseMove × 147` сирим списком | `MOVE (500,300)` — один action |
| Raw `Down + Up + Up + Down` | Авто `CLICK` або `HOLD` |
| `KeyDown E + KeyUp E` окремо | `KEY PRESS E` (або `Ctrl+C` для комбінацій) |
| Manual double-click | **Auto-detect** (opt-in) |
| Delete = втрата | **Disable** (тимчасово вимкнути) |
| Tabular UI | **Cards/blocks** з drag-reorder |

---

## 🛠️ Збірка та Запуск

### Dev-режим (швидка ітерація):
```bash
cd src-tauri
cargo run --release
```

### Release-збірка (рекомендовано):
```bash
cd src-tauri
cargo build --release
```

Файл буде створено у `target/release/NanoClick.exe` (~9.94 MB).

### Windows Installer і signing

Конфіг Tauri налаштований у `src-tauri/tauri.conf.json`. Для створення
installer потрібен **Tauri CLI 2.x** (не 1.x!):

```bash
# одноразово: CLI 2.x
cargo install tauri-cli --version "^2"

# NSIS installer (основний формат)
cargo tauri build
```

Артефакти з'являться у `target/release/bundle/nsis/NanoClick_*-setup.exe`.

Налаштування, які вже стоять у конфігу:
- `webviewInstallMode: downloadBootstrapper` — installer не тягне повний
  WebView2 runtime (економить ~130+ MB), за потреби завантажує його сам;
- NSIS `installMode: currentUser` — встановлення без адмін-прав.

### Оновлення (updater) — дві окремі системи підпису

| Система | Що робить | Статус |
|---|---|---|
| **Tauri updater signing** (minisign keypair) | підтверджує, що update-артефакт наш; безкоштовно | plugin підключений, `check_for_updates` готова, чекає pubkey + endpoint |
| **Windows code signing** (signtool + certificate) | прибирає SmartScreen-попередження; платно | `scripts/sign-windows.ps1` готовий, чекає certificate |

Updater infrastructure можна готувати вже зараз — вона НЕ залежить від
платного сертифіката:

```bash
# 1. згенерувати keypair (ОДИН РАЗ; private key ніколи не комітити і
#    не втрачати — інакше старі інсталяції не довіряють новим оновленням)
cargo tauri signer generate -w ~/.tauri/nanoclick.key

# 2. public key → tauri.conf.json → plugins.updater.pubkey
# 3. tauri.conf.json → bundle.createUpdaterArtifacts: true
#    → build генерує *-setup.exe.sig поруч з installer
# 4. GitHub Releases: installer + .sig + latest.json
#    → plugins.updater.endpoints
```

Детальний план — у [`NEXT_STEPS_ROADMAP.md`](./NEXT_STEPS_ROADMAP.md),
розділ "Installer & update pipeline".

Зараз installer unsigned: реальний Windows certificate thumbprint і timestamp
URL треба додати перед публічним релізом. Підпис без сертифіката навмисно
не імітується.

### Тести:
```bash
cd src-tauri
cargo test --lib --release
# → 57 passed; 0 failed
```

### 🐛 Debugging
У production DevTools вимкнено навмисно. Rust і frontend diagnostics дзеркаляться
у `%TEMP%\nanoclick_web.log`:

```text
type %TEMP%\nanoclick_web.log
```

Для локальної діагностики використовуй debug build і журнал, а не залишай
DevTools увімкненими у release.

---

## 📁 Структура Проекту

```
G:/forder/autocliker_agr/autoclicker_rust/
├── Cargo.toml              ← workspace root (members = ["src-tauri"])
├── src/                    ← frontend (HTML/JS/CSS/assets)
│   ├── index.html          ← 36K chars, повний UI з 7 вкладками
│   ├── main.js             ← ~2000 рядків ES module
│   ├── style.css           ← кнопки, теми, layouts
│   └── NanoClick.png       ← icon
├── src-tauri/              ← Tauri backend
│   ├── Cargo.toml
│   ├── tauri.conf.json     ← withGlobalTauri (no-op у Tauri 2.x), frontendDist: "../src"
│   ├── build.rs            ← rerun-if-changed для frontend
│   └── src/
│       ├── main.rs         ← Tauri entrypoint
│       ├── lib.rs          ← setup + invoke_handler (debug_log тощо)
│       ├── commands/       ← Tauri commands (play_macro_from, step_macro, тощо)
│       ├── core/           ← action.rs, executor.rs, execution.rs, sequence.rs
│       ├── recorder/       ← hooks, normalizer, raw_event
│       ├── platform/       ← windows-specific keyboard, windows_hooks
│       ├── config*.rs      ← persistence + types
│       ├── scheduler.rs    ← click loop, atomic state
│       └── persistence/    ← macros JSON storage
└── target/
    └── release/
        └── NanoClick.exe   ← 9.94 MB Tauri binary
```

---

## 📝 Changelog

### v4.0 (поточний білд) — Advanced Automation
- ➕ **5 нових Action variants:** `Repeat { count, inner }`, `If { condition, then, else }`, `Call { macro_id }`, `SetVar { name, value }`, `GetVar { name, into }`
- ➕ **`Condition` enum:** `True`, `VarEq { name, value }`, `VarLt`, `VarGt`, `PixelEquals { x, y, color, tolerance }`
- ➕ **`ExecutionContext`** — HashMap змінних + call stack + `last_value` register, обмежений `MAX_CALL_DEPTH = 16`
- ➕ **`MacroLookup = Arc<dyn Fn(&str) -> Option<Macro> + Send + Sync>`** — виклик nested макросів через closure
- ➕ **`dispatch_primitive_with_cancel(action, cancel)`** — для chunked cancellation в runner
- ➕ **`play_macro_from(start_idx)` / `step_macro` / `rewind_macro`** Tauri commands для debug
- ➕ Summary counter: `N control-flow · M vars` в Automation tab
- 🔧 `cancel: Arc<AtomicBool>` shared між ExecutorHandle і runner (lock-step stop)
- 🔧 Видалено `src/main.rs` (stale v2 console, shadowed Tauri entrypoint)
- 🔧 Додано `build.rs` rerun-if-changed для frontend → Rust rebuilds тригеряться при зміні JS

### v3.3 — Visual Editor
- ➕ Drag-reorder блоків у macro
- ➕ Context menu ⋮ — Run from here / Step / Disable / Duplicate / Delete
- ➕ Inline-edit значень (wait ms, координати)
- ➕ Re-record button (повний перезапис)
- ➕ ⚡ Optimize 3 рівні (Subtle / Balanced / Aggressive)
- ➕ Recording overlay (translucent red banner)
- ➕ CSS button classes (`.btn-primary`, `.btn-secondary`, `.btn-danger`, `.btn-mini`, `.ve-*`)

### v3.2 — Macro Recorder
- ➕ **14 Action variants:** `MouseMove`, `MouseClick`, `MouseDown`, `MouseUp`, `Scroll`, `Wait`, `KeyPress`, `KeyDown`, `KeyUp`, `HoldStart`, `HoldEnd`, ...
- ➕ **5-phase Smart Normalizer:** KeyDown/Up pairing → MouseDown/Up → Click/Hold detection → Double-click auto-detect → Wait insertion
- ➕ **WH_MOUSE_LL + WH_KEYBOARD_LL** hooks (zero CPU)
- ➕ **Mouse coalescing** (>20 px threshold)
- ➕ **Auto-Wait insertion** between actions
- ➕ **Ctrl+Shift+R** global hotkey для record toggle
- ➕ Macro JSON persistence (`persistence/macros.rs`)

### v3.0.1 — Hotfix & Performance
- 🐛 Hotkey listener: `GetAsyncKeyState` polling → event-driven `WH_KEYBOARD_LL` channel
- 🐛 Jitter `radius_px` — поле було оголошене, але нікуди не передавалось (Pattern 10 / label ≠ impl)
- 🐛 Start/Stop таймери: JS `setTimeout` → Rust worker loop
- ⚡ Presets view: `forEach + createElement` → `innerHTML` + event delegation
- ⚡ Прибрано `transform: translateY(-2px)` (paint reflow на hover)
- ⚡ Transitions 0.18s → 0.12s

---

## 📚 Документація

| Файл | Опис |
|---|---|
| [`docs/MACRO_ARCHITECTURE.md`](./docs/MACRO_ARCHITECTURE.md) | Повна архітектура Action Engine |
| [`docs/v4_CONTROL_FLOW.md`](./docs/v4_CONTROL_FLOW.md) | v4.0 Repeat/If/Call/Variables spec |
| [`UI_UX_SPECIFICATION.md`](./UI_UX_SPECIFICATION.md) | Дизайн-система, теми, sidebar, layouts |
| [`PRODUCT_ROADMAP_COMPARISON.md`](./PRODUCT_ROADMAP_COMPARISON.md) | Roadmap + порівняння з конкурентами |

---

## 🔬 Lessons Learned (dev session, 2026-08-19)

Цей проект пережив 4 сесії «UI рендериться, але нічого не реагує». Уроки:

1. **`<script type="module">` strict mode forbids duplicate top-level declarations.** Один випадковий `function escapeHtml(s)` (через refactor) кидав `Uncaught SyntaxError: Identifier 'escapeHtml' has already been declared` — увесь JS модуль не виконувався. Усі handlers мертві, UI рендериться. Діагноз: відкрити DevTools (F12), перевірити Console.
   ```bash
   node --check --input-type=module < src/main.js   # 100ms syntax check
   grep -nE '^(const|let|var|function)\s+\w+' src/main.js | sort | uniq -d -f1   # find dupes
   ```

2. **Tauri 2.x removed `withGlobalTauri` globals.** `withGlobalTauri: true` у `tauri.conf.json` — no-op. Використовуй `window.__TAURI_INTERNALS__.invoke(cmd, args)` напряму.

3. **Cargo workspace shadowing.** Старий `src/main.rs` (legacy v2 console, 19KB, untouched 2026-08-17) був auto-detected Cargo як binary target і shadowed Tauri entrypoint → Cargo писав console binary замість Tauri. **Завжди перевіряй `target/release/NanoClick.exe` MainWindowTitle** (`"NanoClick v3.0"` означає Tauri WebView; шлях до exe означає console).

4. **Frontend files bundled at compile time.** Rust не бачить `src/main.js` як dependency → зміни JS не тригерять rebuild. `build.rs` має `cargo:rerun-if-changed=../src/main.js`.

5. **Release builds (`windows_subsystem = "windows"`) мають no stderr.** `eprintln!` у `#[tauri::command]` нічого не виводить. Пиши у `std::env::temp_dir().join("nanoclick_web.log")`.

Skill: `silent-noop-fix-debugging` — `references/case-study-nanoclick-syntaxerror.md`

---

© 2026 NanoClick Project. v4.0 — Built with Rust + Tauri 2.
