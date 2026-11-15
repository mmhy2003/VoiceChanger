# VC-141: Setup offers to download and install VB-CABLE

## Goal

When no virtual audio cable is found, setup shows a "Virtual audio cable" page that offers to download VB-CABLE from vb-audio.com and install it. Today it only offers to open the download page at the end. VB-CABLE is never shipped inside our installer.

## Decisions

| Decision | Chosen | Why / rejected |
|---|---|---|
| Where the offer appears | Its own wizard page (your answer) | Rejected: a checkbox on "Select Additional Tasks" (one line of explanation only) and a checkbox on the finish page (download would run after the wizard closes). |
| How VB-Audio's installer runs | Silently, inside our wizard (your answer) | `VBCABLE_Setup_x64.exe -i -h`. The user still sees the UAC prompt and usually Windows Security's "install this device software?" prompt; those cannot be suppressed. Rejected: showing VB-Audio's own window. |
| Restart | Offered only if the cable does not show up after install (your answer) | Rejected: always offering (VB-Audio's wording) and never offering. |
| Which pack | Pinned URL and SHA-256 in `setup.iss` | Same rule as the base models (pinned URL, checked hash). Currently `https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack46.zip`, the pack linked on vb-audio.com/Cable today. A new pack needs a small PR to bump both values. Rejected: scraping the page for the newest link (cannot be hash-checked, breaks when the page changes). |
| Unzipping | Windows' own `{sys}\tar.exe` | Present on every Windows we support (`MinVersion=10.0.19045`). Rejected: Inno's `ExtractArchive` with `ArchiveExtraction=full`, which adds 465 KB, the 7-Zip DLL (a new third-party licence in the installer) and needs Inno Setup 6.5+. |
| When it runs | On "Install" from the Ready page, before VoiceChanger's files are copied | This is Inno's documented place for a download page. A cable failure never stops VoiceChanger from installing. |
| Silent installs | Never download or install the cable | The in-app updater runs setup with `/SILENT`; an update must not pop a UAC prompt. |
| Licence | No bundling; the page names VB-Audio and its donationware terms | VB-Audio's licensing page asks that users can identify VB-CABLE as VB-Audio's and are told it is donationware, with the origin www.vb-cable.com. |

## Architecture

Everything is in `installer/setup.iss` (`[Code]`), following the existing cable check. No Rust code changes.

1. **Offer page.** `InitializeWizard` creates a custom page after `wpSelectTasks` with the text and two radio buttons from the mockup ("Download and install VB-CABLE" is the default). `ShouldSkipPage` hides it when `NoVirtualCable` is false. A silent run has no pages, so nothing is chosen.
2. **Ready page.** `UpdateReadyMemo` adds "Virtual audio cable: Download and install VB-CABLE from vb-audio.com" when it is chosen.
3. **Install step.** `NextButtonClick(wpReady)` calls `InstallCable` when the cable is wanted, then always returns True. `InstallCable` shows a `CreateDownloadPage` page and does, in order:
   - download the pinned zip to `{tmp}` with `TDownloadWizardPage.Add(url, name, sha256)` and `Download` (Inno checks the SHA-256 and raises on a mismatch or an HTTP error);
   - unzip it into `{tmp}\vbcable` with `tar.exe -xf` (hidden, exit code checked);
   - run `VBCABLE_Setup_x64.exe -i -h` with `ShellExec('runas', …, ewWaitUntilTerminated)` from that folder, with the page text "Installing the VB-CABLE audio driver…" and a line saying Windows will ask for approval;
   - re-run `VirtualCableInstalled` once a second for up to 15 seconds.
4. **Outcome**, kept in one variable and logged for the test:
   - `installed`: the cable is detected. Finish page as today, without the download box (the cached `NoVirtualCable` result is refreshed).
   - `restart`: the installer ran but no cable is detected. `NeedRestart` returns True, so Inno shows its "restart now / later" finish page. `[Messages] FinishedRestartLabel` is reworded as in the mockup.
   - `failed`: download, hash, unzip or launch failed. A message box gives the reason and says setup continues. The finish page keeps today's checked "Open the VB-CABLE download page" box.
   - `declined`: the UAC prompt was refused (`ShellExec` error 1223). No message box. Same finish page as `failed`.
   - `skipped`: the user chose "Skip". Same finish page as today.

The existing `[Run]` entry for the download page and the existing detection code stay. `README.md` (Install step 3) and `CHANGELOG.md` (Unreleased) are updated.

## Data and interfaces

- `setup.iss` defines: `CableUrl`, `CableSha256`, `CableSetupExe` (`VBCABLE_Setup_x64.exe`; the plan confirms the name and computes the hash from the pinned pack).
- New setup command-line switches, for `installer/test-install.ps1` only:
  - `/INSTALLCABLE`: do the cable install without the page, also in silent mode.
  - `/CableUrl=` and `/CableSha256=`: replace the pinned values. Accepted only for `http://127.0.0.1:` URLs, so they cannot point setup at another site.
  - `/CableRegistryKey=` already exists and is unchanged.
- Log lines: `VoiceChanger: VB-CABLE installed`, `… restart needed`, `… not installed: <reason>`, `… declined`. The existing `virtual cable found / not found` lines stay.

## Error handling and edge cases

- No network, HTTP error, changed file (hash mismatch), `tar.exe` failure, missing setup exe in the zip: outcome `failed`, VoiceChanger still installs, the user gets the download page link.
- Cancel on the download page: treated as `skipped`; setup carries on with VoiceChanger.
- Driver prompt refused in Windows Security: VB-Audio's installer exits without a cable. This is indistinguishable from "needs a restart", so the restart page also says where to get the cable if it is still missing afterwards.
- A cable that is already present (VB-CABLE, VoiceMeeter, Virtual Audio Cable): the page is skipped, nothing is downloaded.
- `/SILENT` and `/VERYSILENT` without `/INSTALLCABLE`: no download, no UAC, no restart request. The in-app updater is unaffected.
- VoiceChanger itself still installs per user without admin rights. Only VB-Audio's installer is elevated.
- ARM64 PCs: the x64 setup program is used; VB-Audio's manual says it installs the right driver for the running Windows.

## Testing

All automated in `installer/test-install.ps1`, which CI and the release workflow already run. No manual checklist.

- **Fake pack:** the test compiles a small stub named `VBCABLE_Setup_x64.exe` that records its arguments and creates the fake cable endpoint under the HKCU test key, zips it, and serves it from a local HTTP listener on 127.0.0.1.
- **Cases:**
  1. Silent install, no cable, no `/INSTALLCABLE`: nothing is downloaded (the listener gets no request), log says "virtual cable not found".
  2. `/INSTALLCABLE` with the fake pack and the right hash: the stub ran with `-i -h`, the fake endpoint exists, log says "VB-CABLE installed", exit code 0.
  3. `/INSTALLCABLE` with a wrong hash: the stub did not run, log says "not installed", VoiceChanger is installed, exit code 0.
  4. `/INSTALLCABLE` with a stub that creates no endpoint: log says "restart needed" (run with `/NORESTART`, so nothing restarts).
  5. `/INSTALLCABLE` with a cable already present: no request to the listener.
  6. `/CableUrl=` pointing outside 127.0.0.1 is ignored.
- **Pin check:** the test downloads the real pinned URL (with retries), compares its SHA-256 with the pinned hash and checks that the zip contains `CableSetupExe`. A moved or changed pack turns CI red instead of reaching users.
- The existing install, upgrade, relaunch and uninstall checks stay as they are.

## Out of scope

- Shipping VB-CABLE inside our installer.
- Installing the real driver in CI (the Windows Security prompt would hang the runner).
- Uninstalling VB-CABLE when VoiceChanger is uninstalled.
- Offering other cables (VoiceMeeter, Virtual Audio Cable) or the VB-CABLE A+B / C+D packs.
- An "Install VB-CABLE" button inside the app; the Settings tab keeps its download link.
- Following new VB-CABLE packs automatically.
