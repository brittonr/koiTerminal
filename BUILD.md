# How to build koiTerminal
### Build the OS first
The upstream app is designed to be a component of AOSP, and leverages system APIs such as `android.system.virtualmachine.VirtualMachineManager`.
Therefore, it seems that this app cannot be built normally and has to be built with the OS build system.
It used to be possible to build just the app with `UNBUNDLED_BUILD_SDKS_FROM_SOURCE=true TARGET_BUILD_APPS=VmTerminalApp m apps_only dist`,
but that has not been working recently.
If anyone knows why, or if anyone knows how to build this app normally, tips are greatly appreciated.

Please follow the [GrapheneOS build guide](https://grapheneos.org/build) and build the OS for your device model.
Use the version tag that corresponds to the tag in koiterminal.
This needs a beefy machine with preferably 32GB of RAM and ~400GiB of storage (~150GiB to download, ~100GiB to check out, ~120GiB to compile, plus any swap file you create).
On my machine that is not very beefy, compilation from scratch takes half a day.

Using the instructions for `Faster builds for development use only` is fine for development as we don't need to sign the OS,
but that will sign the apk with test keys, which everyone has.
To use your own signature, also run `m otatools-package` to build signing tools.

### Tips for building
1. For the correct version of Node.js, you can use [nvm](https://github.com/nvm-sh/nvm).
2. Yarn can be installed from Node.js: `npm install -g yarn`
3. You may need to manually rename the factory image download: `mv vendor/adevtool/dl/<image>.zip.tmp vendor/adevtool/dl/<image>.zip`
4. You may need `git config --global fetch.fsck.badTimezone ignore`
5. Although `repo` is robust to network failures, it is not robust to running out of storage on your drive.

### Tips for building in Whonix
1. Somehow it needs `git config --global core.symlinks true`
2. Ignore the issue with git rev-parse broken in .mk.
    Add `torsocks_bin=/usr/bin/torsocks` to the `git` line in `vendor/google_devices/$DEVICE/adevtool-version-check.mk`
    To figure out what the issue is, you will ned to run the wrapped command yourself to show all of stdout.
3. For the spike of memory usage, you can use a swap file: `# swapon ~/swap.tmp`

### Build koiTerminal
Please finish building the whole OS before following the rest of this guide.

After building the OS, change the Virtualization package to this repo:
```
cd packages/modules/Virtualization
git remote add koiterminal https://github.com/outlawsanzhang/koiTerminal.git
git fetch koiterminal
git checkout koiterminal
cd ../../..
```

Then build the APK target with `m VmTerminalApp`. Use its fresh APK output, not a
previous APK left in APEX staging. Building an OS image from the complete Koi fork
also embeds Koi as a privileged app; do not use that image to upgrade a device
where Koi must remain a separate application. The native-display profile below
provides a stock-Terminal-preserving OS patch for that case.

### Sign the build
Using the test key means anyone can update your app into anything else.
It is best to sign the apk with your own key.

The new-keystore instructions below are for fresh installations. For an in-place
update, the signing certificate must match the installed APK. Never uninstall or
clear application data to bypass a signer mismatch when preserving an existing VM.

First, generate your keystore if you do not have one.
```
DEVICE=... # fill your device here
CN=... # fill your name here
keytool -genkeypair -alias VmTerminalApp -keyalg RSA -keysize 4096 -validity 10000 -keystore keys/$DEVICE/vm-app-signing.jks -dname "CN=$CN"
```
You can sign the app with apksigner:
```
DEVICE=... # fill your device here
RELEASE_OUT=releases/$BUILD_NUMBER/release-$DEVICE-$BUILD_NUMBER

mkdir -p "$RELEASE_OUT"

cp -- "${APK:?Set APK to the fresh m VmTerminalApp output}" "$RELEASE_OUT/VmTerminalApp.apk"
apksigner sign --ks keys/$DEVICE/vm-app-signing.jks $RELEASE_OUT/VmTerminalApp.apk
```

## VM-owned native display: SDK-36 build and device verification

This path requires an Android/AVF system update **and** a matching Koi APK.
The source and build checks below were followed by a separately approved
`user`-build deployment and physical-device verification on 2026-11-17, recorded
at the end of this section. The initial source-only preparation did not change
the phone; that historical boundary does not describe the later deployment.

### Exact source and packaging boundary

The target is the existing **comet / Android API 36 / GrapheneOS 2026021200**
profile. Build the stock-preserving OS with `comet-cur-userdebug` from the frozen
manifest below: its `cur` configuration is Android 16 / API 36 and selects the
pinned `grapheneos/` kernel. The retained AOSP `bp4a` configuration instead selects
an absent stock-kernel directory and is not the OS build recipe. Do not substitute
the retained 2026062800 APK: its minimum SDK is 37. Lowering that APK's SDK
declaration is not a compatible rebuild.

Pinned inputs:

- GrapheneOS manifest: `7fab44ce395fc2b2624ecca3eaaa9889b76a50d5`.
- Stock `packages/modules/Virtualization`: `77a464d761db97770b901f37f3c8e2d9a4ca4ac8`.
- `system/sepolicy`: `2f75218b85c353130d6c9cf7f322021d4e7c05b2`.
- `frameworks/native`: `a9412b20547812f11233b745aa094dab98468882`.
- Koi APK source base: `5b6e7d6eb069deacf5358367a7d7f52901b889c7`, plus the
  prepared working-tree changes. It descends from the pinned stock AVF revision;
  the affected native/framework files were identical before these changes.

Use [vm-owned-display-platform.patch](patches/vm-owned-display-platform.patch)
for the **stock** AVF module, not a wholesale replacement with the Koi fork. It
contains the native/framework implementation and migrates the stock display
client while retaining `com.android.virtualization.terminal`, its manifest, and
the existing APEX app selection. Koi remains the separate
`com.android.virtualization.koiterminal` data APK; the OS patch does not promote it
to a privileged/system application.

From a full checkout of the pinned Android manifest, with `KOI_SOURCE` set to this
prepared Koi checkout:

```sh
git -C packages/modules/Virtualization apply \
  "${KOI_SOURCE:?}/patches/vm-owned-display-platform.patch"
git -C system/sepolicy apply \
  "${KOI_SOURCE:?}/patches/vm-owned-display-sepolicy.patch"
```

Both patches were checked against clean, exact pinned source. They are generated
candidate artifacts: regenerate the platform patch if its implementation changes;
do not assume it tracks later working-tree edits automatically.

The public `android.system.virtualizationdisplay` kernel-Binder broker checks
both VM permissions and the full allocating UID before returning a CID-specific
display. All five native display methods independently check that UID. Generation
checks protect replacement displays/CIDs from late clear/death notifications.
VM Binder death drops its broker handles but retains a retired CID reservation:
the manager dying does not prove that crosvm exited. Only owner-checked confirmed
exit cleanup releases that reservation; it cannot be adopted even by the same UID.
The policy adds only the broker and required display IPC; it does **not** give
untrusted applications lookup access to the privileged internal VM service.

`VirtualMachine.getDisplayService()` uses the existing `terminal_gui_support` API
flag. The selected product must also enable the existing custom-VM paravirtualized
device support. Native `Surface` handles cannot be forwarded over virtmgr's Binder
RPC connection; acquisition stays on kernel Binder. `setSurface` is an **input**
parameter, and the server retains its own window reference rather than consuming
and attempting to return the caller's Surface.

### Build/signing prerequisites and required gates

A complete SDK-36 platform checkout and its pinned vendor inputs/toolchain are
required; the former matching build tree was not retained. A new frozen checkout
and strict comet vendor generation passed on leviathan on 2026-11-16. The resolved
manifest SHA-256 is `88ef4cdba7463be809c1e98142cef2a9110e73e20b4acc8bc91b90a0dc24bf8c`.
That manifest/vendor record establishes input readiness only; subsequent build
and artifact verification is recorded below. The old API-37 APK is not a
replacement. Budget the full source/build capacity described above, and run one
heavy build at a time.

Build the stock-preserving OS first. Generated vendor configuration otherwise
skips neverallow checks. In the supported local `buildspec.mk`, preserving any
other local settings, enforce:

```make
override SELINUX_IGNORE_NEVERALLOWS := false
```

Required platform commands include:

```sh
export OUT_DIR=out
lunch comet-cur-userdebug
test "$(get_build_var PLATFORM_SDK_VERSION)" = 36
test "$(get_build_var SELINUX_IGNORE_NEVERALLOWS)" = false
m framework-virtualization.stubs.source.system-check-current-api \
  framework-virtualization.stubs.source.system-api-lint \
  virtualizationservice virtmgr crosvm framework-virtualization VmTerminalApp \
  virtualizationdisplay_fuzzer \
  virtualization_display_registry_test vm_display_access_test \
  selinux_policy sepolicy_neverallows sepolicy_tests treble_sepolicy_tests
m vendorbootimage vendorkernelbootimage target-files-package otatools-package
```

Keep `OUT_DIR` relative to the checkout: this pinned Soong test-package graph
rejects absolute host-JAR paths. The API and lint target names above were resolved
from that generated graph, including their actual current-API validation rules.

Execute the two host test binaries; successful compilation alone is not a test
pass. The complete API/lint, Rust/NDK, policy/neverallow and device builds must
pass without weakening SELinux or waiving diagnostics. AVF is non-updatable in
this profile, and the framework and platform policy also change: this is not an
APK-only or piecemeal APEX update.

The public broker has a registered `virtualizationdisplay_fuzzer`, using the
real published Binder service rather than a no-fuzzer exemption. It is
device-only and requires a compatible installed OS; compiling it is not proof
that fuzzing or a Binder transaction has run on the phone.

Build Koi's `VmTerminalApp` separately from the prepared Koi source using the
matching SDK-36 framework. Do not generate replacement OS images from that APK
workspace. Verify the fresh APK's package, minimum SDK, version and certificate
before any separately authorized installation; do not pick a stale stock APK
from earlier APEX staging.

The preserved `2026082001` Koi APK (SHA-256
`6422799682ba5cf769df6162b4fced7ed5b4bc8428b7e17e6f140a237a4e0616`)
passes `apksigner verify`. Its certificate SHA-256 is
`a40da80a59d170caa950cf15c18c454d47a39b26989d8b640ecd745ba71bf5dc`,
matching the pinned Android build's public development `testkey.x509.pem`.
That establishes a development signing path, **not production signing**. An
in-place APK replacement must retain that certificate and must not downgrade
below version code `2026082001`; never uninstall Koi or replace its private VM.
The observed phone is already bootloader-unlocked, but that is neither flashing
authority nor proof of safe data-preserving OS installation.

### Verification performed and remaining runtime boundary

Performed:

- Nine registry regressions passed with warnings denied, including foreign/full
  Android-user UIDs, concurrent VMs, stale generations, failed-create cancellation
  and recovery registration. A failing-before/passing-after case pins CID
  reservation after unconfirmed VM exit. They also passed from the applied
  platform patch.
- The C++ owner/revocation executable passed, including from the applied patch.
- A host registry smoke with real Unix socket handles demonstrated replacement
  cleanup, stale-death refusal and resource closure on VM removal. This is not
  an Android Binder or graphical runtime proof.
- Public broker Java/Rust/NDK and internal Rust/NDK AIDL generation passed.
  Corrected Surface Java/NDK generation used the real pinned native Surface
  declaration; both generated Java interfaces compiled against the API-36 SDK.
  These checks used SDK build-tools 35.0.0 and JDK 21, not a full platform build.
- The final platform patch applied to clean pinned stock source; the separate
  SELinux patch passed its clean-source application check. Twenty SDK input
  files matched the applied platform patch, and seventeen native/framework
  inputs matched the separate Koi source tree.
- The strict SDK-36 comet platform gate passed on leviathan on 2026-11-16
  (attempt r6): current system API check/lint, Rust/NDK and framework/stock-app
  compilation, real display-broker fuzzer link, and policy/neverallow targets.
  Earlier missing-fuzzer and missing-`liblog` link failures remain separate.
- The produced Rust host executable ran all nine named registry regressions;
  the C++ owner/revocation executable passed on both x86_64 and x86. The default
  FHS lacked the x86 interpreter; that distinct run used the pinned SDK glibc
  2.17 loader and matching 32-bit GCC runtime, not the host's 64-bit libraries.
- Full OS packaging and the separate Koi APK build passed on leviathan. The APK
  build used a private root-mapped user/mount namespace to bind the prepared Koi
  module into the SDK view, with `OUT_DIR=out-koi`; it did not replace the host's
  stock source tree or generate OS images from the Koi workspace.
- All 9,933 target-files and 480 OTA-tool archive members passed CRC checks.
  The archived vbmeta RSA-4096 signature and all twelve referenced image
  hashes/hashtrees verified using the embedded key. This is self-consistency,
  not publisher authentication or proof of device acceptance.
- The actual archived AVF payload retains `com.android.virtualization.terminal`;
  its packaged framework DEX defines public `VirtualMachine.getDisplayService()`
  returning `android.os.IBinder`. The separate ARM64 Koi APK is
  `com.android.virtualization.koiterminal`, version `2026082001`, minimum/target
  SDK 36, signed with the matching development certificate above. Its SHA-256 is
  `aa5bf697df270bc47f520bf607fec8a14014493a24b5a3cdf3add00398d6aaa4`.
- An initial byte-equality check between standalone and archived vendor boot
  outputs failed and remains recorded. All 518 vendor ramdisk entries then
  matched in contents and filesystem metadata except 42 non-hard-linked inode
  numbers; AVB metadata also differs. Use the coherent target-files `IMAGES`
  set, not a mixture with standalone outputs.

The ready artifacts, patches, `build-verification.json` and checked `SHA256SUMS`
are on leviathan under
`/home/brittonr/onix-builds/koi-native-display-2026021200-20261116/artifacts/`.
Those initial OS artifacts are development `userdebug/test-keys` outputs with
security patch level `2026-02-05`, not the later `user` build installed on the
phone. The target-files archive includes userdata material and must not be
treated as permission to flash or erase it.

### Approved Fold deployment and physical verification, 2026-11-17

The approved installation retained the existing Koi app UID and private VM data;
it did not uninstall Koi, clear its data, or replace `root_part`. The installed
OS identifies as
`google/comet/comet:16/BP4A.260205.002/2026111601:user/test-keys`, API 36,
`ro.debuggable=0`, SELinux enforcing, security patch `2026-02-05`. Stock
`com.android.virtualization.terminal` remains installed beside Koi. The separately
installed Koi APK matched SHA-256
`aa5bf697df270bc47f520bf607fec8a14014493a24b5a3cdf3add00398d6aaa4`.
This is still a development/test-key OS with an old patch level, not a production
security baseline; do not relock the bootloader on the strength of these tests.

Before installation, the encrypted cold backup was decrypted and restored into
isolated tmpfs: all ZIP members passed CRC, the sparse mapping and required VM
registration files matched, and read-only ext4 verification exited zero. The
retained validation is on leviathan at
`/home/brittonr/onix-backups/koi-fold-20261117T051752939Z/validation-r5.json`.
This was not an on-phone restore, Android keystore restoration, or a boot of the
restored copy. Preserve the backup and its narrower scope.

The original VM metadata was restored, using Koi's existing GUI/input device
overrides rather than replacing that metadata or the guest disk. On the actual
Fold:

- Two real app UIDs allocated separate diskless, networkless crosvm VMs. Each
  owner could call its own native display handle; cross-owner calls, including
  a transferred replacement handle, raised `SecurityException`. Calls through
  stopped and old-generation handles raised `DeadObjectException`. This probe
  did not claim guest OS boots or rendered pixels.
- The production display activity handled an unavailable/stopped VM with an
  error dialog and survived; this was observed separately from the owner probe.
- Weston 15 and `weston-terminal` rendered through Koi's native monitor under
  guest user `droid`, UID 1000. Actual on-screen keyboard taps executed
  `hostname` and displayed `comet-avf`; generic ADB text injection was not counted
  as working keyboard input.
- A separate `weston-eventdemo` displayed its real window and received Wayland
  touch down, motion and release events from Android taps and a swipe. Its
  initial non-default-geometry invocation exited with a Wayland protocol error;
  only the successful default-geometry run counts. ADB mouse injection produced
  no pointer-event witness and is not a passing mouse test.
- After guest `systemctl poweroff`, the host VM list became empty. A Koi cold
  start produced a different guest boot ID, automatically started Weston and
  the graphical terminal, retained the machine-ID/public SSH-host-key hashes and
  the exact `m8r4714fhqhnfkcnxvzhnp9binnpyzh1` system profile with kernel
  `6.1.167`, and reported `running` with zero failed system units. Fresh
  on-screen input in the new native monitor executed `id` as UID 1000.
- Temporary owner-probe and backup-helper APKs were uninstalled. Koi and stock
  Terminal remained installed, SELinux remained enforcing, and the graphical
  terminal was left open. No global Android hidden-API policy was changed.

#### Opening the preserved guest's GUI

Keep Koi's GUI/input override enabled. This guest intentionally disables ttyd:
the default web tab's timeout is not a VM boot failure. Open the **plug/+ serial**
tab first; Koi's existing terminal-readiness UI enables the **monitor** button
when that tab opens. Then open the monitor and its on-screen keyboard. After the
tested cold boot the initial monitor frame still showed a console login; typing
`id` through the on-screen keyboard presented the running graphical terminal.
Immediate correct first paint is not established. Do not treat that old frame
as a request to enter a password.

The existing image supplies `weston.service`. Guest `droid` has lingering enabled;
the added, enabled user unit at
`/home/droid/.config/systemd/user/koi-native-terminal.service` contains:

```ini
[Unit]
Description=Koi native graphical terminal
Requires=weston.service
After=weston.service

[Service]
Environment=WAYLAND_DISPLAY=wayland-0
ExecStart=/run/current-system/sw/bin/weston-terminal --maximized --font-size=24 --shell=/run/current-system/sw/bin/bash
Restart=on-failure

[Install]
WantedBy=default.target
```

This is persisted guest user configuration, not a newly built image/profile.
The original guest disk and serial recovery path remain in use; no
remote-desktop fallback was introduced. These observations cover native Wayland
graphics, software-keyboard input, touch, and a guest/Koi restart. They do not
establish X11 application compatibility, accelerated 3D, physical mouse input,
or virtualization-daemon restart recovery; registry re-adoption after daemon
restart remains model-tested only.

Source-free runtime receipts and captured screenshots/logs are retained beside
the build records on leviathan under
`/home/brittonr/onix-builds/koi-native-display-2026021200-20261116/receipts/`,
including `display-owner-device-r1.json`, `display-unavailable-device-r1.json`,
and `display-native-device-r1.json`. The earlier failed diagnostics and initial
build-only boundaries are not reclassified as passes.

