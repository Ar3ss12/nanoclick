# NanoClick: що далі

Дата: 2026-08-20

## Короткий висновок

Windows-версія вже має основний функціональний набір: автоклікер, режими роботи,
глобальні hotkeys, запис макросів, нормалізацію, оптимізацію, visual builder,
пресети, control-flow та release-збірку.

Але продукт ще не завершений повністю. Наступний етап має бути не просто
перенесенням на Linux/macOS, а стабілізацією контрактів і platform layer.

## Що вже зроблено

- **v1.2.1 Release & Installer Bundle:** усунуто подвійне виклимання `stop()` у записі макросів, впроваджено zero-lock access у системних хуках Windows (`thread_local!`), додано дроселювання мікро-рухів миші (<4px / <30мс), додано 14 нових unit-тестів та зібрано оновлений інсталятор `NanoClick_1.2.1_x64-setup.exe`;
- **v1.2.0 Release & Installer Bundle:** реалізовано персистентний рушій статистики (`StatsConfig` у Rust backend), інтегровано Canvas CPS Timeline графік у реальному часі, впроваджено Smart Keypress Memory (TTL), оптимізовано Sequence Editor Canvas, усунуто витік пам'яті та згенеровано підписаний інсталятор + маніфест оновлень `latest.json` для GitHub Release v1.2.0;
- click engine з режимами single, double, hold та jitter;
- global hotkeys на Windows через `WH_KEYBOARD_LL`;
- комбінації клавіш, включно з NumPad і NumLock-off поведінкою;
- macro recorder з ігноруванням власного record hotkey;
- мінімальний wait 25 ms;
- normalizer та optimizer дій;
- visual builder з редагуванням, перейменуванням і drag-reorder;
- repeat, if/else, variables та nested macro calls;
- presets з основними налаштуваннями engine;
- release build без DevTools у production;
- Rust unit tests для core, recorder, persistence та hotkey parser.
- 95 passing Rust tests, включно з legacy-config migration, corrupted macro
  JSON, 2,000-action macro roundtrip та physical SendInput hotkey integration.

## Поточний scope: Windows-first

Повну кросплатформеність зараз не беремо в продуктову розробку. Поточна
ціль - завершити Windows-продукт і підготувати межі для майбутніх backend-ів.
Linux/macOS не повинні блокувати v4.0-v4.2.

### v4.0 Macro Engine + Presets UI - завершено

Цей етап завершено. Основні критерії виконані:

1. Macro Engine стабільно виконує repeat, if/else, variables та nested calls.
2. Recorder коректно оптимізує події, зберігає мінімальний wait 25 ms і не
   записує власні start/stop hotkeys.
3. Visual builder дозволяє переглядати, редагувати, перейменовувати та
   reorder-ити дії.
4. Presets зберігають усі налаштування головного екрана, підтримують create,
   view, edit, apply, import та migration старого формату.
5. Повторний start/stop, cancellation, corrupted JSON і довгі записи покриті
   базовими тестами та готові до post-release перевірки.

### v4.1 Windows Stability - завершено (2026-08-24)

Мета етапу - не додавання нових великих функцій, а стабілізація вже готової
Windows-версії перед наступною архітектурною роботою.

1. [x] Integration-тести з фізичними keydown/keyup: одиночні клавіші,
   modifier-комбінації, NumPad (усі 10 keypad VK + NumLock-off aliases),
   key-up anti-refire та зміна binding без restart.
2. [x] Parse-once contract для hotkeys: listener більше НЕ парсить рядки на
   кожному poll (було 100 разів/сек). Scheduler веде `hotkeys_version`
   (AtomicU64), бамп при кожному save; listener перечитує bindings тільки
   коли версія змінилась. Рядки залишились лише для UI/persistence.
3. [x] Listener diagnostics: hook installed/failed, binding parsed,
   last event, fired action (всі 7 дій), reject reason.
4. [x] Довгі записи, cancellation, повторний start/stop, corrupted JSON -
   покриті тестами; single-instance делегований tauri_plugin_single_instance
   (+ тест double-start guard на GLOBAL_HOTKEY_RUNNING).
5. [x] Single-instance lock, cleanup hooks/channel, release log policy з
   rotation; shutdown idempotency test.
6. [x] Windows installer pipeline ПОВНІСТЮ: NSIS installer, updater keypair,
   createUpdaterArtifacts (.sig), GitHub Release v1.0.0 (latest) з installer
   + .sig + latest.json, pubkey + endpoint вбудовані в tauri.conf.json,
   updater АКТИВНИЙ (in-app update bar, автоперевірка кожні 6 год,
   signed download+install+relaunch). Залишилось ТІЛЬКИ Windows code signing
   (чекає сертифікат, scripts/sign-windows.ps1 готовий).

Стан тестів: 66 passing Rust tests.

Технічні нотатки v4.1 closure:
- фізичний capture-hook у тестах зроблено restartable (`ensure_hook_alive`):
  Windows може мовчки зняти LL-hook після інтенсивного SendInput; canary-кей
  перевіряє доставку і перевстановлює хук за потреби (раніше другий
  physical-тест у ран отримував порожній канал);
- message pump більше не помирає на першій помилці GetMessageW.

### v4.2 Platform Abstraction - ЗАВЕРШЕНО (2026-08-24)

Це не порт на Linux/macOS, а рефакторинг меж у Windows-коді.

#### Що ВЖЕ є (аудит знайшов готову базу - не робити вдруге)

- `platform/backend.rs`: traits `InputBackend` / `HotkeyBackend` /
  `RecorderBackend` вже існують (позначені `#![allow(dead_code)]`, бо
  ніхто їх не реалізує);
- `recorder/raw_event.rs`: уніфікований `RawEvent` enum вже існує
  (MouseMove / MouseDown / MouseUp / KeyDown / KeyUp / Scroll / WaitGap,
  кожен з `t_ms`);
- `windows_hooks.rs` ВЖЕ емітить нейтральні `RawEvent` з `KeyCode(vk)`;
- core на ~95% чистий: 0 `use windows::` в `core/*`, `KeyCode(pub u16)`
  нейтральний; єдині "VK_xx" згадки - в human-readable назвах (це ок);
- free functions мають однакові сигнатури в platform/{windows,linux,macos}.

#### Реальна пов'язаність, яку треба прибрати

1. **executor.rs: 14 `#[cfg]` блоків** викликають
   `platform::windows::{mouse_down, mouse_up, scroll_wheel, send_key,
   set_cursor_pos, mouse_click}` напряму + дублікати no-op тіл під
   `cfg(not(windows))`. Це головне місце роботи.
2. **scheduler.rs: stringly-typed API** -
   `click_mouse_ext(&str button, "hold", &str pos_mode, x, y, jitter)`.
3. **lib.rs / commands**: `spawn/shutdown_global_hotkey_listener`,
   `stop_recorder_hooks`, `spawn_recorder_hooks` - вільні функції замість
   backend-об'єктів.
4. **linux/macos stubs** мовчки повертають `false`/no-op без повідомлення.

#### Розбіжності між traits і реальною архітектурою (вирішити ДО рефакторингу)

- `HotkeyBackend::register/unregister` не відповідає реальній моделі:
  Windows-реалізація - це LL-hook + matcher loop зі snapshot rebinding
  (v4.1 parse-once), а НЕ per-binding register. Trait треба переглянути
  під цю модель (`set_bindings(all)` + event stream), інакше абстракція
  буде брехливою.
- `RecorderBackend::start(_, ignored_hotkey: String)` - stringly;
  замінити на типізований binding.
- `PlatformCapabilities` не існує - додати.

#### Порядок кроків v4.2 (кожен крок зелений на тестах)

#### Прогрес (2026-08-24)

- [x] **Крок 1 — WindowsBackend**: `WindowsBackend` реалізує `InputBackend`
  (mouse_click/down/up, scroll_wheel, set_cursor_pos, send_key,
  cursor_position, click_mouse(ClickSpec), release_mouse_hold);
  executor отримує backend через конструктор і містить **нуль**
  `#[cfg(target_os)]` блоків; no-op дублікати видалено. Бекенд вибирається
  тільки в `platform::default_input_backend()`.
- [x] **Крок 2 — Типізований scheduler API**: `ClickSpec { button,
  click_type: ClickType, position_mode: PositionMode, fixed_x, fixed_y,
  jitter_radius }` у `platform/backend.rs`; scheduler парсить рядки конфіга
  ОДИН раз на початку worker-потоку і далі несе типізаний spec;
  stringly `click_mouse_ext`/`release_mouse_hold` видалені з windows mod.
- [x] **Крок 3 — HotkeyBackend redesign**: trait тепер `start()/stop()/
  is_running()` (відповідає реальній hook+snapshot моделі) + `HotkeyAction`
  enum для майбутнього event stream; `WindowsHotkeyBackend { scheduler,
  app_handle }` реалізує трейт; lib.rs стартує/стопає через
  `platform::default_hotkey_backend(...)`, а не вільні функції.
- [x] Contract test `all_primitives_route_through_input_backend_contract`
  (mock backend рахує диспетчі). **Тести: 60/60** (паралельно і serial).
- Критерій виконано: `grep "platform::windows"` в core/, scheduler.rs,
  commands/, lib.rs → **0 збігів**.

#### Решта кроків

4. **RecorderBackend**: start приймає типізований ignored binding;
   commands йдуть через trait object.
5. **PlatformCapabilities**
   `struct WindowsBackend;` що реалізує `InputBackend`. executor отримує
   `&dyn InputBackend` (або generic param) замість 14 cfg-блоків.
   No-op дублікати під cfg(not(windows)) видаляються.
2. **Типізувати scheduler API**: `ClickSpec { button: MouseButton,
   click_type: ClickType, pos_mode: PositionMode, fixed_x, fixed_y,
   jitter_radius }` замість рядків; парсинг рядків конфіга лишається
   тільки на кордоні (config_manager).
3. **HotkeyBackend redesign**: trait `set_bindings(snapshot) -> ()` +
   `events() -> Receiver<HotkeyAction>`; WindowsBackend реалізує через
   існуючий listener loop. lib.rs викликає метод backend, не вільну fn.
- [x] **Крок 4 — RecorderBackend**: `WindowsRecorderBackend` у
  windows_hooks.rs реалізує трейт; ignored-hotkey label парситься на межі
  платформи; commands (record_start/stop/cancel + Drop) йдуть через
  `platform::default_recorder_backend(...)` / `recorder_backend_stop()`.
- [x] **Крок 5 — PlatformCapabilities**: `{global_hotkeys,
  global_input_recording, mouse_injection, keyboard_injection}` +
  `can_play_macros()`; `detect()` — all-true на Windows, all-false на
  stubs; нова Tauri-команда `get_platform_capabilities` для UI.
- [x] **Кроки 6–7 — Contract tests**: 6 нових тестів у backend.rs
  (парсинг ClickType/PositionMode/HotkeyAction з конфіг-міток, mock
  input backend приймає typed ClickSpec, mock recorder lifecycle,
  capabilities contract). Linux/macOS НЕ портовано — stubs лишились
  capability-honest. **Тести: 66/66** (parallel + serial).

### v4.2 ЗАВЕРШЕНО (2026-08-24)

Фінальний аудит-прохід (того ж дня):
- linux.rs / macos.rs: видалено старі stringly заглушки
  (`click_mouse_ext`, `release_mouse_hold`,
  `spawn/shutdown_global_hotkey_listener`, `stop_recorder_hooks`) —
  вся функціональність тепер ТІЛЬКИ через contracts + Noop-бекенди;
  лишилось timer/stop-event plumbing;
- windows/mod.rs: прибрано мертвий тест-хелпер `inject_and_drain`,
  загашено `unused_must_use` на `UnhookWindowsHookEx`; нуль warning'ів;
- **Поведінковий фікс:** старий stringly `click_mouse_ext` ігнорував
  параметр `_click_type` — режим "Double" в UI мовчки робив один клік.
  Типізований `WindowsBackend::click_mouse` тепер справді доставляє
  подвійний клік (2 кліки з паузою 50ms).
- **UI capability surface:** фронтенд викликає `get_platform_capabilities`
  на старті (`checkPlatformCapabilities()` у main.js); якщо
  `can_play_macros=false` — червона dismiss-панель знизу + вимкнений
  toggle-батон; окремі попередження для global_hotkeys /
  global_input_recording. Команда повертає serde_json з полем
  `can_play_macros` для прямого використання в JS.

Критерій виконано: `grep "platform::windows"` в core/, scheduler.rs,
commands/, lib.rs → 0 збігів; executor не містить жодного
`#[cfg(target_os)]`; всі входження через trait objects + фабрики в
platform/mod.rs. Наступний етап: v4.3 (Linux X11 backend під ті самі
contract tests).

Критерій завершення v4.2: `grep -r "platform::windows" core/ scheduler/
commands/ lib.rs` → 0 збігів; всі виклики через trait objects; тести
зелені без жодного `#[cfg]` всередині core/. 

### 1. Зафіксувати Windows-реліз

1. Додати інтеграційні тести hotkeys: одиночна клавіша, modifier-комбінація,
   NumPad, повторне натискання, key-up та зміна binding без перезапуску.
2. Перестати використовувати сирі строки для hotkeys у внутрішньому коді.
   Зберігати окремо `modifiers`, `trigger`, physical/logical key та формат для UI.
3. Додати діагностику стану listener: hook installed, binding parsed,
   last event, last fired action, причина відхилення комбінації.
4. Прибрати застарілі згадки в README: 42/42 замінити на актуальний результат,
   а опис release DevTools узгодити з тим, що DevTools вимкнено.
5. Перевірити довгі записи макросів, скасування, повторний start/stop,
   corrupted JSON та одночасний запуск кількох екземплярів.

### 2. Ресурси та безпека

1. Додати single-instance lock, щоб два listener-и не працювали одночасно.
2. Перевірити cleanup hook/channel при shutdown.
3. Обмежити debug logging у release та додати ротацію log-файлу.
4. Додати code signing для Windows installer.
5. Додати updater лише після стабільного формату конфігів і міграцій.

#### 2a. Installer & update pipeline - конкретний план (уточнено 2026-08-22)

Дві системи підпису - НЕ плутати:

```text
Windows code signing (signtool / certificate)
  → підтверджує видавця NanoClick.exe / Setup.exe перед SmartScreen
  → потребує платний certificate (або Azure Trusted Signing)

Tauri updater signing (minisign keypair)
  → підтверджує, що update artifact виданий нами
  → безкоштовно, генерується `cargo tauri signer generate`
  → public key вбудовується в tauri.conf.json, private key → CI secret
```

Порядок робіт:

1. NSIS installer (основний формат для користувачів):
   - `tauri.conf.json` → `"bundle": { "targets": ["nsis"] }`
     (зараз стоїть `"all"` — звужуємо, WiX/MSI не потрібен);
   - `webviewInstallMode: downloadBootstrapper` вже стоїть — лишаємо,
     інакше installer роздується на ~130-180 MB;
   - `installMode: currentUser` вже стоїть — лишаємо;
   - перевірити: install, uninstall, upgrade поверх старої, запуск без
     dev-оточення, clean install на машині без Rust/Node.
2. Tauri updater signing (можна робити вже зараз):
   - `cargo tauri signer generate` → keypair;
   - private key → локальний secret / CI secret (НИКОЛИ не комітити;
     втрата ключа = старі інсталяції ніколи не довіряють новим оновленням);
   - public key → `plugins.updater.pubkey` (зараз порожній);
   - `"bundle": { "createUpdaterArtifacts": true }` → build генерує
     `*-setup.exe.sig` поряд з installer.
3. Update endpoint (після пункту 2):
   - GitHub Releases як статичний хостинг: installer + `.sig` + `latest.json`
     (формат static JSON endpoint з офіційної документації);
   - `plugins.updater.endpoints` → URL на latest.json (зараз порожній);
   - команда `check_for_updates` у lib.rs вже написана — запрацює сама,
     коли endpoint і pubkey будуть заповнені.
4. Windows code signing (останній крок, коли буде certificate):
   - `scripts/sign-windows.ps1` вже готовий (signtool + sha256 + timestamp);
   - заповнити `certificateThumbprint` і `timestampUrl` у tauri.conf.json
     (зараз null) або підписувати скриптом після bundle.

Що НЕ робити:
- не активувати updater у UI до появи реального endpoint з підписаними
  артефактами (перевірка підпису в Tauri обов'язкова — з порожнім pubkey
  check просто впаде, це безпечно);
- не вбудовувати повний WebView2 runtime у installer без причини;
- не генерувати MSI/WiX, поки немає enterprise-вимоги.

### 3. Архітектура

Виділити спільні backend traits:

```text
InputBackend
  mouse_click / mouse_down / mouse_up
  key_press / key_down / key_up
  cursor_position

HotkeyBackend
  register / unregister / event stream

RecorderBackend
  start / stop / event stream
```

Windows реалізація залишиться на native hooks. Linux і macOS матимуть власні
модулі, а core executor, normalizer, persistence та UI не повинні знати про
`VK_*`, Win32 handles або X11 types.

## Пізніший backlog портів

### Після v4.2: Linux X11

Починати з X11 і явно документувати це як першу підтримувану конфігурацію.
Для Tauri 2 потрібні системні WebKitGTK 4.1 залежності. Для AppImage треба
будувати на найстарішій цільовій системі або в CI-контейнері, інакше новіша
glibc може зробити binary несумісним зі старими дистрибутивами.

План Linux:

1. `platform/linux/input.rs` для mouse/keyboard input.
2. `platform/linux/hotkeys.rs` через `global-hotkey` або Tauri global-shortcut.
3. X11 global hotkeys та recorder.
4. Тести на Ubuntu/Debian baseline.
5. AppImage + Debian package.
6. Окреме рішення для Wayland: compositor portal, desktop-specific backend
   або чітке повідомлення про обмеження.

Wayland не можна вважати автоматично сумісним із Windows hook-моделлю.
Станом на roadmap-дослідження universal global hotkeys там залежать від
desktop/compositor і upstream-підтримки.

### Після Linux X11: macOS

Потрібні окремі native реалізації для event tap/input simulation та системний
дозвіл Accessibility. Global hotkey backend на macOS має працювати на main
thread з активним event loop.

План macOS:

1. `platform/macos/input.rs` через CoreGraphics/AppKit bindings.
2. Accessibility permission check з поясненням для користувача.
3. Global hotkeys на main thread.
4. Recorder та mouse/keyboard simulation.
5. Apple Silicon build.
6. Universal binary Intel + Apple Silicon.
7. Code signing, notarization та DMG.

## Що не потрібно робити зараз

- не переписувати frontend на React/Vue без вимірюваної причини;
- не додавати десятки нових control-flow блоків до стабілізації існуючих;
- не обіцяти повну Wayland-підтримку без тестів на GNOME/KDE;
- не робити updater до появи signed artifacts і schema migrations;
- не виносити hotkey logic у JS: це погіршить надійність global input.

## Рекомендований план релізів

### v4.0 Macro Engine + Presets UI - завершено

Macro/preset workflow, recorder, optimizer, builder та persistence завершені.
Залишається лише виправлення регресій, знайдених під час v4.1.

### v4.1 Windows Stability - зараз

Physical hotkey integration tests, lifecycle, diagnostics, logging, migrations
та installer pipeline.

### v4.2 Platform Abstraction - після v4.1

Відокремлення Windows implementation від core, unified events та contract tests.

### Пізніше: v4.3 Linux X11

Перший реальний порт: X11 input/hotkeys/recorder, AppImage/deb і CI.

### Пізніше: v4.4 macOS

CGEventTap, permissions onboarding, universal build, signing, notarization та DMG.

### Пізніше: v5.0 Cross-platform polish

Capability matrix, updater після schema migrations і signed artifacts, Wayland
як experimental/limited backend та спільна cross-platform quality matrix.

## Архітектурний аудит портів

Цей розділ уточнює roadmap після перевірки моделі глобального вводу. Головний
ризик для NanoClick не в Tauri, а в тому, що Windows, Linux і macOS мають
різні правила доступу до keyboard/mouse events. Тому Windows-specific hooks
не можна рознести по `#[cfg]` у core-коді; потрібен стабільний platform contract.

### Hotkey, recorder і input simulation - різні підсистеми

Їх не слід об'єднувати в один backend:

- `HotkeyBackend` визначає, чи спрацювала конкретна комбінація.
- `RecorderBackend` віддає потік усіх дозволених keyboard/mouse events,
  включно з подіями поза активним вікном.
- `InputBackend` відтворює натискання, рух миші, down/up і повертає позицію
  курсора.

Tauri plugins можуть допомогти з окремими функціями, але не замінюють повний
platform layer. Core має спілкуватися з Rust backend, а не з `WH_KEYBOARD_LL`,
`VK_*`, X11 handles чи CoreGraphics types.

### Єдиний потік подій

Цільова межа між платформою та core:

```text
Raw platform events
        |
Platform normalizer
        |
Unified InputEvent
        |
Recorder -> optimizer -> persistence/executor
```

Мінімальна модель `InputEvent` повинна покривати `MouseMove`, `MouseDown`,
`MouseUp`, `KeyDown`, `KeyUp` і `Scroll`, з timestamp та фізичним ключем там,
де це можливо. Normalizer та optimizer не повинні знати, чи подія прийшла з
Win32, X11, evdev або CoreGraphics.

Внутрішній hotkey також має бути структурованим:

```text
HotkeyBinding {
  modifiers,
  physical_key,
  logical_key,
  display_name,
}
```

Рядок потрібен лише для UI, імпорту та backward-compatible persistence. Він не
має бути основним типом у matcher, scheduler або platform backend.

### Capability model

Підтримку треба показувати за можливостями, а не одним прапорцем `Linux` або
`macOS`:

```text
PlatformCapabilities {
  global_hotkeys,
  global_input_recording,
  mouse_injection,
  keyboard_injection,
  permission_state,
}
```

Це дозволить чесно повідомити користувачу, що саме недоступне: наприклад,
macOS без Input Monitoring/Accessibility або Wayland без повного recorder.
Backend повинен мати перевірку capabilities після надання дозволу, а UI -
показувати дію `Grant permission`, а не мовчазне неробоче записування.

### Бібліотеки-кандидати

`enigo` можна розглянути як adapter для input simulation на Windows, macOS та
Linux/X11. Це може зменшити кількість native-коду, але не повинно потрапити у
core API: його capabilities, Wayland-обмеження та відмінності key mapping треба
сховати за `InputBackend` і перевірити contract tests.

`monio` цікавий як reference для global monitoring/recording/playback, але його
не слід додавати лише через обіцянку єдиного API. Спочатку потрібно перевірити
підтримувані платформи, permissions, event ordering, latency, cleanup і
Wayland-поведінку. Рішення про dependency приймається після v4.2 abstraction,
а не під час v4.0.

Висновок: бібліотеки можуть бути реалізаціями майбутніх adapters, але не мають
визначати модель `InputEvent`, persistence або macro engine.

### Платформні висновки

#### Windows

Windows залишається reference implementation: native low-level hooks для
подій, `SendInput` для відтворення, окремі lifecycle та cleanup. Потрібні
справжні integration-тести з фізичними keydown/keyup, бо matcher unit tests
не перевіряють встановлення hook, message loop, порядок подій і cleanup.

#### Linux X11

Це перший реалістичний Linux-реліз: окремі `linux/x11` backends для hotkeys,
recording та injection. X11 Record може бути дослідницьким варіантом, але
реалізацію треба обрати після перевірки актуальних бібліотек, latency та
permissions; не прив'язувати core до конкретного X11 API. AppImage слід
будувати на baseline з найстарішою підтримуваною glibc через CI або container.

#### Linux Wayland

Wayland не є ще одним X11 backend. Compositor/portal може надати global
shortcuts через XDG GlobalShortcuts, але це не означає доступ до всіх
keyboard/mouse events для recorder. `evdev`/`uinput` працюють на рівні
фізичних пристроїв і вимагають окремих permissions, udev rules та security
рішень; вони не роблять Wayland автоматично повністю сумісним.

На першому релізі треба декларувати `Wayland: limited`, тестувати GNOME/KDE
окремо і не обіцяти повний recorder, поки немає підтвердженого backend.

#### macOS

Recorder базується на `CGEventTap`, input simulation - на CoreGraphics, з
окремими перевірками Input Monitoring та Accessibility. Перший запуск має
вести користувача до System Settings і після зміни permission повторно
перевіряти capabilities. Це окремий onboarding flow, а не помилка recorder.

### Спільний contract test suite

До реалізації нового backend потрібно зафіксувати contract tests для:

- left/right/middle/X1/X2 click;
- mouse move та cursor position;
- key press, key down/up і modifier-комбінацій;
- hotkey register/unregister та зміни binding без перезапуску;
- recorder start/stop, повторного start/stop і cancellation;
- довгого запису, corrupted JSON та відновлення після помилки.

Ті самі семантичні тести мають запускатися для Windows, Linux X11 і macOS.
Wayland повинен мати capability-aware тести, які перевіряють коректне
повідомлення про недоступну функцію, а не штучно очікують Windows-поведінку.

### Реальний стан реалізації на 2026-08-20

Вже є:

- `HotkeyCombo`/matcher і базова перевірка modifier-комбінацій;
- listener diagnostics: parsed binding, last event, fired action,
  reject reason;
- single-instance lock та cleanup під час завершення;
- 59 Rust tests, включно з corrupted JSON, hotkey regression cases і великим
  macro roundtrip;
- release log policy з rotation;
- початкові `InputBackend`, `HotkeyBackend`, `RecorderBackend` traits;
- updater plugin/config skeleton і Windows signing script.

Стан installer/update pipeline (2026-08-22 — ПЕРШИЙ РЕЛІЗ-АРТЕФАКТ ЗБІРАЄТЬСЯ):

- tauri-cli оновлено до **2.11.4** (був 1.6.6);
- `bundle.targets = ["nsis"]`, `createUpdaterArtifacts = true` — застосовано;
- updater keypair згенеровано: private → `~/.tauri/nanoclick.key`
  (НИКОЛИ не комітити і не втрачати), public → вбудований у tauri.conf.json;
- перший installer успішно зібраний і підписаний:
  `target/release/bundle/nsis/NanoClick_3.0.0_x64-setup.exe` (~3.4 MB)
  + `NanoClick_3.0.0_x64-setup.exe.sig`;
- команда збірки з підписом:
  `TAURI_SIGNING_PRIVATE_KEY=<path> cargo tauri build --bundles nsis`
  (змінна TAURI_SIGNING_PRIVATE_KEY_PATH у CLI 2.11.4 ігнорується при
  bundle-signing — використовувати саме TAURI_SIGNING_PRIVATE_KEY);
- NSIS 3.11 та nsis_tauri_utils завантажуються Tauri автоматично при
  першій bundle-зборі — вручну ставити нічого не треба;
- залишилось: GitHub Releases endpoint + latest.json (updater стане активним),
  Windows code signing (чекає certificate, sign-windows.ps1 готовий).

Що залишалось незавершеним станом на 2026-08-20 — СТАТУС 2026-08-24:

- [x] фізичні Windows integration-тести keydown/keyup (59 tests, NumPad включно);
- [ ] concrete backend adapters, які реально використовують спільні traits (v4.2);
- [~] hotkey strings лишаються для UI/persistence; matcher/listener працюють
  на типізованих HotkeyCombo з parse-once contract (v4.1 done);
- [x] live updater endpoint та public key (GitHub Release v1.0.0 + latest.json,
  updater активний);
- [ ] справжнє code signing без сертифіката (чекає certificate);
- [x] фактична збірка NSIS installer у середовищі з Tauri CLI 2.x (працює).
### Фінальний пріоритет

1. **v4.0 Macro Engine + Presets UI:** завершено; приймати лише regression fixes.
2. **v4.1 Windows Stability:** ЗАВЕРШЕНО ✅ (2026-08-24) — physical hotkey
   tests, NumPad, lifecycle, diagnostics, parse-once bindings, installer +
   активний updater (Release v1.0.0). 59/59 tests. Наступний етап: v4.2.
3. **v4.2 Platform Abstraction:** наступний етап — прибрати 14 cfg-блоків
   з executor, типізувати scheduler API, redesign HotkeyBackend під реальну
   hook+snapshot модель (план адаптовано за аудитом 2026-08-24, див. розділ
   v4.2). База вже є: traits і RawEvent існують, core ~95% чистий.
4. **Пізніше:** Linux X11, macOS, а потім Wayland limited/capability-based.

Порядок installer/update робіт (незалежно від сертифіката):

```text
Крок 1: tauri-cli 2.x + targets ["nsis"] → перший Setup.exe
Крок 2: cargo tauri signer generate → keypair
        (private → secret, public → tauri.conf.json)
Крок 3: createUpdaterArtifacts: true → .sig поруч з installer
Крок 4: GitHub Releases + latest.json → updater активний
Крок 5: (коли буде certificate) sign-windows.ps1 → SmartScreen-чистий installer
```

Отже, найближче завдання - не порт на три ОС, а завершення Windows contract і
конкретної abstraction. Після цього X11 буде окремою реалізацією контракту, а
Wayland залишиться capability-limited продуктом.

## Джерела дослідження

- Tauri distribution: https://tauri.app/distribute/
- Tauri global shortcut plugin: https://tauri.app/reference/javascript/global-shortcut/
- `global-hotkey` platform notes: https://github.com/tauri-apps/global-hotkey
- Tauri Linux prerequisites: https://tauri.app/start/prerequisites/
- Tauri AppImage compatibility notes: https://tauri.app/distribute/appimage/
- Tauri macOS signing/notarization: https://tauri.app/distribute/sign/macos/
- Apple CGEvent: https://developer.apple.com/documentation/coregraphics/cgevent
- Apple Input Monitoring: https://support.apple.com/guide/mac-help/control-access-to-input-monitoring-on-mac-mchl4cedafb6/mac
- X11 Record extension: https://www.x.org/releases/X11R7.7/doc/recordproto/record.html
- XDG Global Shortcuts: https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html
- Wayland global shortcut issue: https://github.com/tauri-apps/plugins-workspace/issues/3267
- Rust pynput reference: https://docs.rs/crate/pynput/latest
- Enigo input simulation: https://github.com/enigo-rs/enigo
- Monio input monitoring reference: https://github.com/HuakunShen/monio


---

## Дослідження: проблеми хоткеїв і зупинки (2026-08-25, RESEARCH ONLY)

### Симптоми (від юзера)
1. Швидке подвійне натискання toggle — клікер іноді ПРОДОВЖУЄ працювати
   (миттєво вимкнути не можна).
2. Реакція на включення по хоткею — забагато часу; хочеться налаштування
   (винести в меню / додати до пресетів).
3. Клікер блокує/сповільнює ІНШІ хоткеї в системі.
4. Після вимкнення лишаються "мікро додаткові" кліки; зупинка не миттєва.

### Знайдені причини (по коду)

**A. Головний підозрюваний для #1 і #3: синхронний файл-лог у LL-hook циклі.**
`run_keyboard_hook` на КОЖНУ подію клавіатури пише 1–2 рядки через
`debug_log_internal` ("info"/"stage-ok"/diag) — а `debug_log_internal` у
release пропускає тільки level=="info"; "stage-ok" і diag пишуть у файл +
eprint!. WH_KEYBOARD_LL — СИНХРОННИЙ: поки наш callback не повернеться,
вся клавіатура системи чекає. Під навантаженням (запис у %TEMP%,
rotation, eprintln) це дає затримки вводу по всій системі = симптом #3.
21 release-active лог-викликів у windows/mod.rs.

**B. Race при швидкому тапі (#1).**
Канал подій + fallback-полінг GetAsyncKeyState (recv_timeout 100ms):
- якщо UP губиться (швидкий тап, або hook пропуск), `held` залишається
  з ключем => наступний DOWN бачить was_held=true і НЕ стріляє =>
  toggle "з'їдений", стан інвертується — юзер тисне ще раз і клікер
  знову вмикається замість вимкнення;
- fallback-скан може синтезувати повторний down, якщо recv_timeout
  истёк між down і up (стан гонки з реальним up у черзі).

**C. Мікро додаткові кліки після стопу (#4).**
- Клік dispatch відбувається ДО перевірки active на наступній ітерації
  => рівно 1 зайвий клік після сигналу stop (несинхронізовано);
- ClickType::Double: sleep(50ms) всередині click_mouse без перевірки
  cancel => ще до 2 кліків;
- jitter робить SetCursorPos на кожен клік — останній клік зміщує
  курсор вже після рішення юзера зупинитись.

**D. Затримка реакції (#2).**
Ланцюг: hook -> channel -> listener -> hotkey_toggle -> set_active ->
spawn_worker -> стартовий emit. Плюс перший клік чекає повний інтервал
(next_click = now + interval). Ніякого debounce/delay налаштування в
конфігу немає (перевірено).

### План виправлень (кожен крок окремо, тести зелені)

1. ✅ **Тихий hook-thread (фікс #3) — ВИКОНАНО (коміт 621f4c9):**
   - усі per-event debug_log видалені з while-циклу listener'а
     (toggle/record/heartbeat/reject/re-parsed → ring buffer);
   - діагностика тепер у пам'яті (`HOTKEY_DIAG` VecDeque, cap 128,
     `hotkey_diag_dump()` для тестів/дебагу) — нуль syscalls на клавішу;
   - keyboard_proc: try_lock fast-path (при контенції подія пропускається,
     fallback GetAsyncKeyState її відновлює);
   - regression-guard тест `hook_loop_contains_no_file_logging` сканує
     джерело і падає, якщо файл-лог повернеться на гарячий шлях.
   Тести: 67/67.
2. **Toggle debounce + state-based рішення (#1):** замість сліпого
   toggle — цільова дія за актуальним станом (if active { stop } else {
   start }) + мінімальний інтервал між toggle (наприклад 150мс,
   конфігурований) + примусовий synthesize-up: якщо DOWN без UP довше
   500мс і ключ фізично відпущений (GetAsyncKeyState=false) — чистити held.
3. **Миттєвий стоп (#4):** перевіряти active ПЕРЕД кожним click_mouse;
   Double-тип — розкласти на 2 одиночних з перевіркою cancel між ними;
   опційно "стоп-скасовує останній jitter" не робити (шкідливо), але
   останній SetCursorPos можна пропускати якщо !active.
4. **Налаштовувана реакція (#2):** нові поля конфігу hotkey_debounce_ms
   (default ~100-150) + start_delay_override; UI — селект у головній
   панелі поруч із пресетами ("Hotkey response: instant/fast/relaxed").
5. **Тести:** детермінований тест на race down/up/down (mock годинник),
   тест що hook-цикл не робить file IO (static assert / code scan test).

Статус: ДОСЛІДЖЕНО, не реалізовано (за запитом юзера).