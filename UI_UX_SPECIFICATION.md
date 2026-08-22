# 🎨 NanoClick — Специфікація Продуктового Дизайну та Інтерфейсу (UI/UX Specification)

Цей документ описує концепцію інтерфейсу, дизайн-систему та логіку взаємодії користувача з програмою **NanoClick v4.0**.

> **Поточна версія:** v4.0 (зібрано `cargo build --release` — ~9.94 MB exe, 57/57 tests ✅)
> **v3.0.1 зміни:** Preset cards оптимізовано — `forEach + createElement` → `innerHTML` + event delegation, прибрано `transform` на hover (був причиною paint lag), додано XSS-safe `escapeHtml()` для імен пресетів.
> **v3.2 додано:** новий розділ **Automation** із двома шляхами створення макросу (Record / Build), My Macros list, Visual Editor з inline-edit та drag-reorder.
> **v3.3 додано:** ⚡ Optimize 3 рівні, context menu ⋮ (Run from here / Step / Disable / Duplicate), Re-record button.
> **v4.0 додано:** Variable Inspector counter в sidebar (`N control-flow · M vars`), блоки для `Repeat`/`If`/`Call`/`SetVar`/`GetVar` у Visual Editor з collapsible розгортанням.

> **v3.0.1 зміни:** Preset cards оптимізовано — `forEach + createElement` → `innerHTML` + event delegation, прибрано `transform` на hover (був причиною paint lag), додано XSS-safe `escapeHtml()` для імен пресетів.
>
> **v3.2 план:** новий розділ **Automation** із двома шляхами створення макросу (Record / Build), My Macros list, Visual Editor з inline-edit та drag-reorder.

---

## 💎 1. Філософія Продукту

Продукт орієнтований на створення максимального комфорту, візуальної естетики та впевненості користувача під час автоматизації рутинних завдань:

1. **Мінімалістична естетика:** Сучасний Glassmorphism з м'якими розмиттями та глибокими темними відтінками.
2. **Захист від помилок:** Режим `Work Mode` запобігає випадковим спрацьовуванням гарячих клавіш при роботі з іншими програмами.
3. **Гнучкість персоніфікації:** Теми та акцентні кольори під будь-який настрій чи робочий простір.
4. **Швидкодія:** Hotkey listener через `WH_KEYBOARD_LL` hook — CPU = 0 коли немає натискань; presets рендер через event delegation — O(1) listener незалежно від кількості пресетів.
5. **"Don't program, perform" (v3.2):** Користувач не вчить мову макросів — він **показує** програмі що робити (через Record), потім уточнює.

---

## 🎨 2. Дизайн-Система та Теми (Themes & Color Engine)

Додаток підтримує 6 вбудованих тем оформлення:
- **Dark Cyberpunk** (Класичний неоновий ціан)
- **Neon Grass** (Яскравий смарагдовий зелений)
- **Dark Slate** (Стриманий графітово-сірий)
- **Midnight Blue** (Глибокий синій)
- **Dracula Crimson** (Шляхетний темно-червоний)
- **Amethyst Purple** (Насичений фіолетовий)

Також доступна палітра швидкозмінних акцентних свотчів для тонкого налаштування кольорів кнопок та рамок.

---

## 📐 3. Структура Сторінок Інтерфейсу

### 3.1 � Dashboard (Головна сторінка)
- **Головний перемикач (Start/Stop):** Кнопка великого розміру з текстовим індикатором та підказкою гарячої клавіші.
- **Слайдери швидкості (CPS) та рандомізації (Jitter):** Дозволяють легко встановити бажаний темп від 1 до 100 кліків/сек.
- **Режими кліків та позиції:** Перемикання між Single, Double, Hold та вибором фіксованої точки на екрані.

### 3.2 ⌨️ Hotkeys (Гарячі клавіші)
Окрема сторінка для швидкого налаштування зручних комбінацій:
- Запуск / Зупинка клікера
- Перемикання режимів роботи
- Екстрена зупинка
- Збільшення та зменшення швидкості (+/- 1 CPS)
- Захоплення точних координат курсора
- **🔴 Record toggle (v3.2):** `Ctrl+Shift+R` (default, configurable)

**Технічна реалізація:** `SetWindowsHookExW(WH_KEYBOARD_LL)` + `GetMessageW` loop. Альтернативи через `/` або `|` у тексті хоткея (напр. `R / K`, `F6|F7`).

### 3.3 ⚙️ Settings (Налаштування)
- **Системний шлях:** Перегляд папки збереження конфігурації з кнопкою прямого відкриття у Провіднику (📁).
- **Поведінка системи (Behavior):** Запуск разом із Windows, згортання у трей, сповіщення та автопауза.
- **Зовнішній вигляд (Appearance):** Вибір теми та акцентного кольору.
- **🆕 Recorder behavior (v3.2):**
  - Mouse movement during recording:
    - `●` Record clicks only (default — рекомендовано)
    - `○` Simplified path (Move тільки коли позиція змінилась > 20 px)
    - `○` Record every movement (raw, для replay-accuracy)
  - Overlay position: top-right / top-left / follow cursor.

### 3.4 🎯 Presets (Менеджер пресетів) — v3.0.1 оптимізовано
- **Сітка карток:** grid layout з rounded cards (border-radius: 10px), accent line згори.
- **Кнопки дій:** `⚡ Застосувати` (primary), `👁️ Інспектор`, `✏️ Редагувати`, `🗑️ Видалити`.
- **Швидкий рендер:** одне `innerHTML` write замість per-card `createElement`.
- **Event delegation:** один listener на grid container (замість 4×N listeners).
- **Hover ефект:** `border-color` + `box-shadow` (compositor-only, без `transform`).
- **XSS-safe:** `escapeHtml()` для імен пресетів.

### 3.5 🎬 Automation — НОВИЙ РОЗДІЛ (v3.2)

> **📌 Усі макроси зберігаються і керуються в цій вкладці.** Це єдине місце для створення, перегляду, редагування, тестування та видалення макросів. Інтеграція з Presets: presets (конфіг engine) і macros (послідовності дій) — різні речі, обидва живуть у sidebar, але не змішані.

**Головний екран:**

```
AUTOMATION                                              [ 🗂 All macros ▼ ]
──────────────────────────────────────────────────────────────────────────

[ 🔴 Record new macro ]    [ ＋ Create manually ]


My Macros
──────────────────────────────────────────────────────────────────────────

🎬 Minecraft routine
   12 actions · 4.2 sec                          [ ▶️ ] [ ✏️ ] [ 🗑 ]

🎬 Testing workflow
   34 actions · 12.8 sec                         [ ▶️ ] [ ✏️ ] [ 🗑 ]

🎬 Fishing bot
   6 actions · 2.0 sec                           [ ▶️ ] [ ✏️ ] [ 🗑 ]
```

**Розташування в sidebar (з `index.html`):**
```
Dashboard → Presets → Automation → Settings → Hotkeys → Statistics → About
        ↑              ↑
        └── 2-й ───────┴── 3-й (одразу праворуч від Presets, окрема вкладка)
```

Тобто Automation — це **3-й пункт sidebar**, **поряд** з Presets, але **не всередині** Presets. Це окрема повноцінна вкладка з власним контентом (Record / Build entry-points + My Macros list).

**Макроси не експортуються в Presets** (і навпаки). Це різні концепції:
- **Preset** = конфіг engine (CPS, jitter, hold, repeat, hotkeys) для звичайного клікера.
- **Macro** = послідовність Actions (Move/Click/Key/Scroll/Wait) для recorder/automation.

Один preset може "запускати" один macro через окремий hotkey (`▶️ Run macro` у Preset detail), але це вже v4.0. У v3.2 macros живуть самостійно в Automation tab.

**Два шляхи створення макросу:**

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

**🔴 Record flow:**
1. Click `🔴 Record new macro` → одразу починається запис.
2. Overlay з'являється top-right:
   ```
   ┌────────────────────────────────────────┐
   │ 🔴 RECORDING     00:04.27              │
   │ Ctrl+Shift+R again to stop             │
   │ 7 actions captured                     │
   └────────────────────────────────────────┘
   ```
3. Користувач виконує дії вручну (у будь-якому вікні, не тільки в NanoClick).
4. `Ctrl+Shift+R` → запис зупиняється → відкривається Visual Editor з чернеткою.

**＋ Create manually flow:**
1. Click `＋ Create manually` → відкривається порожній Visual Editor.
2. Користувач додає Actions через dropdown `[ ＋ Add Action ▼ ]`:
   - 🖱 Mouse Move
   - 🖱 Mouse Click
   - 🖱 Mouse Down
   - 🖱 Mouse Up
   - ⌨️ Key Press
   - ⌨️ Key Down
   - ⌨️ Key Up
   - 🖱 Scroll
   - ⏱️ Wait

### 3.6 ✏️ Visual Editor (v3.3 — наступний після v3.2)

```
Minecraft routine      [ ▶️ Play ] [ ⏭️ Step ] [ 🔴 Re-record ] [ 💾 Save ]  [⋮]
─────────────────────────────────────────────────────────────────────────────────

☰  🖱 MOVE                           [☑️ enabled] [ 🗑 ] [⋮]
    X [ 500 ]   Y [ 300 ]

☰  🖱 LEFT CLICK                    [☑️ enabled] [ 🗑 ] [⋮]
    Single

☰  ⏱️ WAIT                          [☑️ enabled] [ 🗑 ] [⋮]
    [ 300 ] ms

☰  🖱 RIGHT CLICK                   [☑️ enabled] [ 🗑 ] [⋮]
    (750, 400)

☰  ⌨️ KEY PRESS                     [☑️ enabled] [ 🗑 ] [⋮]
    [ E ]

─────────────────────────────────────────────────────────────────────────────────

[ ⚡ Optimize ]  [ ＋ Add Action ▼ ]
```

**Interactions:**
- Click on inline value (`[ 300 ]`, `X [ 750 ]`) → inline edit з авто-селектом.
- Drag `☰` handle to reorder (HTML5 DnD, smooth animation).
- `🗑` — остаточне видалення action.
- `☑️ enabled` — тимчасово вимкнути action без видалення (3-state: enabled / disabled / deleted).
- `[⋮]` — context menu: Run from here / Step / Edit / Duplicate / Disable / Delete.
- `＋ Add Action ▼` — dropdown: Mouse Move / Click / Down / Up / Key / Scroll / Wait.
- `▶️ Play` — запускає macro у поточному стані (навіть якщо не saved).
- `⏭️ Step` — single-step debug mode (див. §3.8).
- `🔴 Re-record` — з confirm dialog (втрачає edits якщо не saved).
- `💾 Save` — updates `Macro.updated_at` і persists.
- `⚡ Optimize` — post-recording Normalizer pass (3 рівні агресивності).

**Empty state:**

```
You don't have any macros yet.

[ 🔴 Record new macro ]    [ ＋ Create manually ]

Macros will appear here. Don't program — perform once, then refine.
```

### 3.7 🎬 Context menu ⋮

Click на `[⋮]` на блоці → dropdown:

```
┌─────────────────────┐
│ ▶️ Run from here    │
│ ⏭️ Step             │
│ ✏️ Edit             │
│ 📋 Duplicate        │
│ ☑️ Disable          │  ← toggle, не видаляє
│ ─────────────────── │
│ 🗑 Delete           │
└─────────────────────┘
```

- **Run from here** — виконує macro починаючи з цього action (debugging).
- **Step** — single-step з pause після цього action.
- **Duplicate** — копія зручна для послідовних Wait (wait 300 → wait 300 → wait 300).
- **Disable** — тимчасово вимкнути (3-state разом з чекбоксом).
- **Delete** — остаточне видалення.

### 3.8 ⏭️ Step execution — debug mode

Click `⏭️ Step` або з context menu → single-step mode:

```
[ ⏹️ Stop ]  [ ⏸️ Pause ]  [ ⏭️ Step ]  [ ▶️ Run ]     Phase: STEP
─────────────────────────────────────────────────────────────────────

✓  🖱 MOVE       (500, 300)         ← done (зелений border)
✓  🖱 LEFT CLICK                     ← done
▶  ⏱️ WAIT         300 ms            ← CURRENT (accent border + glow)
○  🖱 RIGHT CLICK                    ← pending
○  ⌨️ KEY         E                  ← pending
```

Поточний блок виділяється accent кольором + glow shadow. Step виконує **тільки поточний** action і зупиняється.

### 3.9 ⚡ Optimize button

Кнопка `[ ⚡ Optimize ]` запускає **Normalizer поверх вже записаного Macro** (sub-3-second, sync):

**Що робить:**
- Прибирає MouseMove які ближче 20 px від попереднього або від наступної значущої події.
- Об'єднує Down + Up → Click (Hold якщо > 200 ms gap).
- Об'єднує KeyDown + KeyUp → KeyPress (< 50 ms gap).
- Округлює Wait < 25 ms → drop.
- Округлює Wait > 60000 ms → split на 60-сек блоки.
- Detect double-clicks (opt-in).
- Coalesce послідовні Wait values: `Wait 100 + Wait 100` → `Wait 200`.
- Видаляє trailing Wait.

User може вибирати агресивність у Settings → `Optimize aggressiveness: Subtle | Balanced | Aggressive`.

### 3.10 ℹ️ About (Про програму)
Натхненна сторінка з описом переваг продукту, його можливостей та інформацією про версію.

---

## 🧪 4. Performance Characteristics (v3.0.1)

| Операція | v3.0.0 | v3.0.1 |
|---|---|---|
| Hotkey listener CPU (idle) | Низьке навантаження (polling 20 Hz) | — |
| Preset grid render (50 пресетів) | ~50 ms | ~5 ms (1 innerHTML + 1 delegation listener) |
| Preset hover reflow | Так (transform) | Ні (border/shadow only) |
| Start delay / Stop time precision | ±100 ms (JS setTimeout) | ±1 ms (Rust `WaitableTimerEx`) |
| `applyPreset()` field coverage | ~50% | 100% |

---

## 🎯 5. UX Innovations (v3.2)

### 5.1 "Don't program, perform" — paradigm shift
Замість форм `Add Action → Mouse → Click → Coordinate (X, Y) → Add Wait → Enter 300 ms → ...`:
```
[ 🔴 Record ] → робиш вручну → маєш готовий редагований макрос
```

### 5.2 Mouse simplification
Розумний фільтр, який не створює 50 000 Move actions із hi-res миші:
- **Record clicks only** (default): макрос = Click + Wait + KeyPress
- **Simplified path**: Move тільки при дельті > 20 px
- **Record every movement**: raw (для advanced)

### 5.3 Two paths, same Action list
Record і Build створюють **однаковий** формат — Action list у Macro. Жодного "recorded version" vs "manually created version" розгалуження.

### 5.4 Multi-Point безкоштовно
Те, що раніше було окремим MultiPositionManager, тепер — це просто `Move → Click → Move → Click → Move → Click`. Жодного окремого модуля.

---

## 📜 Ліцензія
© 2026 NanoClick Project. Built with Rust + Tauri 2. All rights reserved.
