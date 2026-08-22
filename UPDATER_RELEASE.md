# Signed updater release checklist

The updater integration is compiled into the app, but it intentionally has no
release endpoint or public key until the release channel is created.

Required secrets/configuration:

- `TAURI_SIGNING_PRIVATE_KEY`: private key used by `tauri signer sign`;
- updater public key in `src-tauri/tauri.conf.json`;
- HTTPS endpoint returning Tauri's platform update manifest;
- signed `.nsis.zip`/`.msi.zip` artifacts and matching `.sig` files;
- `SIGNTOOL_PATH`, `WINDOWS_CERTIFICATE_THUMBPRINT`, and
  `WINDOWS_TIMESTAMP_URL` for Windows Authenticode signing.

After those values exist, configure `plugins.updater.endpoints` and `pubkey`,
then expose `check_for_updates` from the UI. Never ship an updater with an
empty public key or unsigned artifacts.

