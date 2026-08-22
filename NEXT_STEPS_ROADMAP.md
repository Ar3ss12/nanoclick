# NanoClick: що далі

Дата: 2026-08-20

## Короткий висновок

Windows-версія вже має основний функціональний набір: автоклікер, режими роботи,
глобальні hotkeys, запис макросів, нормалізацію, оптимізацію, visual builder,
пресети, control-flow та release-збірку.

Але продукт ще не завершений повністю. Наступний етап має бути не просто
перенесенням на Linux/macOS, а стабілізацією контрактів і platform layer.

## Що вже зроблено

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
- 57 passing Rust tests, включно з legacy-config migration, corrupted macro
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

### v4.1 Windows Stability - поточний етап

Мета етапу - не додавання нових великих функцій, а стабілізація вже готової
Windows-версії перед наступною архітектурною роботою.

1. Додати справжні integration-тести з фізичними keydown/keyup для одиночних
   клавіш, modifier-комбінацій, NumPad, key-up та зміни binding без restart.
2. Довести до кінця structured hotkey model: рядок залишити лише для UI та
   persistence compatibility, а matcher/scheduler/backend перевести на тип.
3. Завершити listener diagnostics: hook installed, binding parsed, last event,
   last fired action і reject reason.
4. Перевірити довгі записи, cancellation, повторний start/stop, corrupted JSON
   та одночасний запуск кількох екземплярів.
5. Залишити single-instance lock, cleanup hooks/channel і release log policy
   з rotation; перевірити їх у shutdown tests.
6. Підготувати Windows installer pipeline. Реальне signing виконати лише після
   отримання сертифіката. Updater infrastructure (keypair, endpoint, .sig
   artifacts) можна готувати вже зараз: Tauri updater використовує власну
   криптографію (minisign), яка не залежить від платного Windows code-signing
   certificate. Активувати сам updater для користувачів лише після появи
   стабільного GitHub Releases endpoint.

### v4.2 Platform Abstraction - після стабілізації

Це не порт на Linux/macOS, а рефакторинг меж у Windows-коді:

1. Core не імпортує Win32 types, `VK_*`, `HWND`, `SendInput` або hook handles.
2. Реалізувати concrete `WindowsBackend`, який використовує спільні traits.
3. Додати `InputEvent` та pipeline `raw event -> normalizer -> recorder`.
4. Додати contract tests для input, hotkey і recorder semantics.
5. Описати capabilities та permission state без фіктивних Linux/macOS
   реалізацій.
6. Зберегти platform modules як майбутні точки розширення, але не додавати
   незавершені stubs у продуктову матрицю підтримки.

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
- 52 Rust tests, включно з corrupted JSON, hotkey regression cases і великим
  macro roundtrip;
- release log policy з rotation;
- початкові `InputBackend`, `HotkeyBackend`, `RecorderBackend` traits;
- updater plugin/config skeleton і Windows signing script.

Стан installer/update pipeline (перевірено 2026-08-22):

- `tauri-plugin-updater` підключений, команда `check_for_updates` написана;
- `plugins.updater.pubkey` та `endpoints` у tauri.conf.json порожні — updater
  безпечний, але неактивний;
- `bundle.targets = "all"` — треба звузити до `["nsis"]`;
- `createUpdaterArtifacts` не заданий — `.sig` файли не генеруються;
- `scripts/sign-windows.ps1` готовий, чекає certificate thumbprint;
- tauri-cli 1.6.6 у середовищі — для bundle-збори потрібен CLI 2.x
  (`cargo install tauri-cli --version "^2"`), інакше `tauri build` не збере
  v2-проєкт.

Ще не можна вважати завершеним:

- фізичні Windows integration-тести keydown/keyup;
- concrete backend adapters, які реально використовують спільні traits;
- повне прибирання hotkey strings з config/UI/scheduler через міграції;
- live updater endpoint та public key;
- справжнє code signing без сертифіката;
- фактична збірка NSIS/MSI installer у середовищі з Tauri CLI 2.x та NSIS/WiX.
### Фінальний пріоритет

1. **v4.0 Macro Engine + Presets UI:** завершено; приймати лише regression fixes.
2. **v4.1 Windows Stability:** physical hotkey tests ✅ (57/57), lifecycle,
   diagnostics ✅; залишилось — installer pipeline (див. розділ 2a) і migrations.
3. **v4.2 Platform Abstraction:** відокремити Windows implementation від core,
   додати unified events і contract tests.
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
