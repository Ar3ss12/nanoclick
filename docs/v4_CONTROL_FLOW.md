# 🔁 NanoClick v4.0 — Control Flow Specification

> **Філософія:** Macro — це не просто список `Click → Wait → Click`. Це **програма з гілками, циклами та змінними**, яка залишається читабельною в Visual Editor.

Цей документ описує 5 нових `Action` variants у v4.0:
- `Repeat { count, inner }` — цикл
- `If { condition, then, else }` — розгалуження
- `Call { macro_id }` — підпрограма
- `SetVar { name, value }` — присвоєння
- `GetVar { name, into }` — читання

та `ExecutionContext` (стан виконання макросу).

---

## 1. 📋 Action::Repeat

```rust
Action::Repeat {
    count: i32,        // -1 = infinite, 0 = skip, N > 0 = execute N times
    inner: Vec<Action>,
}
```

**Семантика:**
- `count = -1` → безкінечний цикл (зупиняється через cancel flag)
- `count = 0` → повний no-op (inner не виконується жодного разу)
- `count > 0` → виконує `inner` рівно `count` разів
- Між ітераціями — pause = 0 ms (для таймінгу вставляй `Action::Wait` всередину)

**Inline-блок:** `inner` є `Vec<Action>`, тож `Repeat` може бути вкладеним без runtime overhead (вкладеність обмежена лише стеком, не рекурсією).

**Visual Editor:** рендериться як collapsible block з відступом для `inner` actions. Кнопка `🔁 Repeat ×5` показує лічильник, клік розгортає/згортає.

**Приклад use-case:** «клікнути 10 разів з інтервалом 500 ms»
```rust
Action::Repeat {
    count: 10,
    inner: vec![
        Action::MouseClick { button: MouseButton::Left, count: 1 },
        Action::Wait { ms: 500 },
    ],
}
```

---

## 2. ❓ Action::If + Condition

```rust
Action::If {
    condition: Condition,
    then_branch: Vec<Action>,
    else_branch: Option<Vec<Action>>,  // None = silent no-op
}

enum Condition {
    True,                              // завжди true
    VarEq { name: String, value: i64 },
    VarLt { name: String, value: i64 },
    VarGt { name: String, value: i64 },
    PixelEquals {
        x: i32,
        y: i32,
        color: [u8; 4],                // RGBA
        tolerance: u8,                 // per-channel Euclidean distance
    },
}
```

**Семантика:**
- Обчислюється `condition` поточного стану `ExecutionContext`
- Якщо `true` → `then_branch`, інакше `else_branch` (якщо є)
- `else_branch: None` — це нормально (умовний `if` без `else`)
- `Condition::PixelEquals` читає курсорну позицію через `windows::Win32::UI::WindowsAndMessaging::GetPixel`

**Visual Editor:** рендериться як `❓ If [cond] then ... else ... end`. Для `PixelEquals` показує `📍 (x,y) ≈ #RRGGBB±tol`.

**Обмеження (v4.0):**
- `OnScreenText` (OCR) — **не реалізовано**, у планах
- `TimeOfDay` — **не реалізовано**, у планах (використовуй `Date.now()` у JS)

---

## 3. 📞 Action::Call

```rust
Action::Call {
    macro_id: String,  // назва макросу в MacroStore
}
```

**Семантика:**
- Шукає макрос `macro_id` через `MacroLookup` closure (надається `app_state.macros` під час execution)
- Якщо не знайдено → `eprintln!("Call: macro '{}' not found — skipping")`, **continue** (не fail!)
- Якщо знайдено → виконує його `actions` через `run_actions_in` (recursive call)
- **Глибина обмежена `MAX_CALL_DEPTH = 16`** — на 17-му рівні кидає warning (запобігає нескінченній рекурсії)

**Visual Editor:** рендериться як `📞 Call → <macro_id>` з іконкою-стрілкою. Клік відкриває цільовий макрос у read-only preview.

**Recursive protection:** `Call` додає `macro_id` у call stack `ExecutionContext.calls`, перед поверненням — pop. Якщо `macro_id` вже в стеку — warning + skip.

---

## 4. 💾 Action::SetVar / GetVar

```rust
Action::SetVar { name: String, value: i64 }
Action::GetVar { name: String, into: LastValueRegister }

enum LastValueRegister { LastValue }  // singleton, останнє прочитане значення
```

**Семантика:**
- `SetVar { name: "loops", value: 5 }` → `ctx.variables["loops"] = 5`
- `GetVar { name: "loops", into: LastValue }` → `ctx.last_value = ctx.variables["loops"]`
- Якщо `GetVar` читає неіснуючу змінну → повертає 0 (default)
- `last_value` — це register (не змінна); використовується в `Condition::VarEq { name: "last", value: 5 }` для порівняння з конкретним числом

**Use-case:** лічильник циклів
```rust
vec![
    Action::SetVar { name: "loops".into(), value: 0 },
    Action::Repeat {
        count: 10,
        inner: vec![
            Action::SetVar { name: "loops".into(), value: 1 },  // +1 кожну ітерацію
            Action::MouseClick { ... },
        ],
    },
    // після: ctx.variables["loops"] = 10
]
```

> **TODO (Variable Inspector UI):** відображати `ctx.variables` в real-time під час execution. Поточний `updateSummary()` показує лише counter (`5 control-flow · 3 vars`).

---

## 5. 🗂️ ExecutionContext

```rust
pub struct ExecutionContext {
    pub variables: HashMap<String, i64>,
    pub call_stack: Vec<String>,        // для виявлення рекурсії
    pub last_value: i64,                 // singleton register
}

pub const MAX_CALL_DEPTH: usize = 16;
```

**Lifecycle:**
- Створюється на початку `run_from_to(macro, 0, len)` — окремий для кожного macro run
- Передається через всі nested calls (`Call` shares the same context, не створює новий)
- Знищується при завершенні root macro

**Thread safety:** `ExecutionContext` **не Send** (mutable borrow через `&mut ctx`) — runner виконується в одному потоці. Якщо потрібен concurrent execution — потрібен per-thread context (не реалізовано).

---

## 6. ⚙️ Implementation Details

### `run_actions_in(actions, ctx, cancel, macro_lookup, depth)` signature

```rust
pub fn run_actions_in(
    actions: &[Action],
    ctx: &mut ExecutionContext,
    cancel: Arc<AtomicBool>,                 // shared cancel flag
    macro_lookup: MacroLookup,               // Arc<dyn Fn(&str) -> Option<Macro>>
    depth: usize,                            // current call depth (starts 0)
)
```

**Cancellation:** перевіряється `cancel.load(Ordering::Relaxed)` на початку кожної ітерації top-level loop та перед кожним primitive action. Control-flow actions (`Repeat`/`If`/`Call`) перевіряють cancel **між** своїми inner actions, не всередині них (викликаючий перевіряє, чи внутрішній цикл ще працює).

### `dispatch_primitive_with_cancel(action, cancel)`

Розширена версія `dispatch_primitive` яка приймає `Arc<AtomicBool>` для cancellation. Використовується для primitive actions (`MouseMove`, `KeyPress`, etc.) під час control-flow execution.

### `MacroLookup = Arc<dyn Fn(&str) -> Option<Macro> + Send + Sync>`

Вирішення проблеми "recursion limit while instantiating" з `fn(&str) -> Option<Macro>` через boxed dyn closure:
```rust
pub type MacroLookup = Arc<dyn Fn(&str) -> Option<Macro> + Send + Sync>;
```

Використання:
```rust
let no_macros: MacroLookup = Arc::new(|_: &str| None);
// або
let lookup: MacroLookup = Arc::new(|id: &str| app_state.macros.iter().find(|m| m.id == id).cloned());
```

---

## 7. 🧪 Testing (42 tests, 0 failed)

Усі control-flow paths покриті `core/execution.rs::tests`:

| Test | Що перевіряє |
|---|---|
| `repeat_executes_inner_n_times` | `Repeat { count: 5 }` → 5 кліків |
| `repeat_with_count_zero_is_noop` | `count: 0` → 0 кліків |
| `if_with_true_condition_executes_then` | `If { cond: True, then: [...click], else: None }` |
| `if_with_false_condition_executes_else` | `If { cond: VarEq false, then: [], else: [...click] }` |
| `call_resolves_through_macro_lookup` | `Call` → lookup повертає macro → execute |
| `call_with_missing_macro_skips` | `Call` → lookup returns None → no-op + warning |
| `setvar_then_getvar_roundtrip` | SetVar("x", 5); GetVar("x") → last_value = 5 |
| `cancel_stops_run` | repeat з wait → cancel flag → exit |

Запуск: `cargo test --lib --release` → `42 passed; 0 failed (0.10s)`

---

## 8. 🚧 Limitations та TODOs

- **PixelEquals reads cursor position only**, not arbitrary (x,y). For arbitrary pixel sampling, the implementation uses Win32 `GetPixel(hdc, x, y)` — currently only reads from primary monitor DC.
- **OnScreenText** (OCR через Tesseract) — **не реалізовано**.
- **TimeOfDay** — **не реалізовано** (use `Date.now()` JS-based action as workaround).
- **Variable Inspector UI** — тільки counter в summary, real-time values не видно під час execution.
- **MAX_CALL_DEPTH = 16** — фіксована константа. Якщо потрібно глибше — параметризуй через config.
- **No multi-threading** — runner single-threaded. Concurrent macros потребують per-thread contexts.

---

## 9. 📚 Related Documents

- [`MACRO_ARCHITECTURE.md`](./MACRO_ARCHITECTURE.md) — базова Action Engine (v3.2)
- [`README.md`](../README.md) — загальний огляд проєкту
- [`core/execution.rs`](../src-tauri/src/core/execution.rs) — implementation
- [`core/executor.rs`](../src-tauri/src/core/executor.rs) — ExecutorHandle, dispatch_primitive_with_cancel

---

© 2026 NanoClick Project. v4.0 Control Flow Spec.
