# Cross-platform installers and release distribution

**Status:** Planned and deliberately deferred. Do not start this work until the Product Alpha roadmap reaches **P12** and the first Product Alpha release has been published, except for a packaging/security problem that blocks P11 or P12.

This document is a handoff for the agent that picks up distribution work after the current Product Alpha roadmap. It is a plan, not an ADR: packaging tools may change. Any choice that changes OSP's security, identity ownership, always-on behaviour or update trust model should get an ADR before implementation.

## Goal

A tagged OSP release should produce normal installers/packages for the home node instead of asking ordinary users to understand Rust binaries or a shell installer.

The first vertical slice is deliberately small:

1. **Windows x86_64:** a normal `.exe` installer.
2. **Debian/Ubuntu x86_64:** a `.deb`.
3. **Debian/Ubuntu arm64:** a `.deb`, including Raspberry Pi-class home nodes.
4. Keep the current raw Linux archives and `install.sh` as a fallback until the replacements have passed clean-machine upgrade tests.

After that works, add RPM packages and a generic Linux fallback. Windows arm64 and additional Linux formats come later when there is demonstrated demand.

## Constraints inherited from the architecture

Packaging must preserve these decisions; it is not allowed to quietly redefine them.

- **The home node is always on.** Per [ADR 0006](decisions/0006-desktop-first-always-on-node.md), the node starts at boot without a person logging in, reconnects after failures and is expected to remain running.
- **The identity belongs to the person, not the installation.** Recovery words, signing material, the node database, posts, media, trusted relationships and settings must survive an application update and, by default, an uninstall. Deleting the node's data is a separate explicit action.
- **Program files and node data are separate.** Replacing an installed executable must never replace or reset the node home.
- **One node home, one running node.** Existing lock/duplicate-instance behaviour must continue to work.
- **Setup must not create an identity as root or as an installer service account.** Package post-install scripts may install files and service support, but identity creation/recovery belongs to the person.
- **P11's usability gate may not regress.** A normal user must not be sent back to developer commands just because the package format changed.
- **OSP still needs its media/control dependencies.** Today that means Tor, ffmpeg/ffprobe and HEIC support through libheif/heif-convert. Packaging must either provide them or make the platform package manager provide compatible versions.
- **Updates need a trust model stronger than a checksum downloaded beside the binary.** SHA256 checksums remain useful for corruption detection, but automatic updates must verify signed metadata/artifacts.

## Target formats and order

| Order | Platform | Artifact | Support intent |
| --- | --- | --- | --- |
| 1 | Windows x86_64 | `OSP-Setup-<version>-x64.exe` | Primary Windows installer |
| 1 | Debian/Ubuntu x86_64 | `open-social-p2p_<version>_amd64.deb` | Primary Linux desktop/server package |
| 1 | Debian/Ubuntu arm64 | `open-social-p2p_<version>_arm64.deb` | SBC/mini-PC package |
| 2 | Fedora/RHEL-family x86_64 | `open-social-p2p-<version>.x86_64.rpm` | Native RPM package |
| 2 | Fedora/RHEL-family arm64 | `open-social-p2p-<version>.aarch64.rpm` | ARM RPM package |
| 2 | Generic Linux x86_64 | AppImage or a retained self-contained archive | Fallback, not the preferred always-on install |
| Later | Windows arm64 | Windows installer | Add after x86_64 is proven |
| Later | Arch Linux | native package/PKGBUILD | Community/demand driven |
| Later | Flatpak / Snap | only after a service/sandbox design | Do not make these the first route for an always-on node |

Flatpak and Snap are intentionally not first-line targets. OSP is a persistent network service with a node home, helper processes and boot-start requirements; a sandboxed desktop package should be added only when it preserves those semantics cleanly.

## Packaging tool direction

Recheck tool capabilities when this work actually starts.

Two tools are good candidates today:

- [cargo-packager](https://docs.crabnebula.dev/packager/) is Rust-friendly and currently supports Windows NSIS/WiX installers plus Debian, AppImage and Pacman packages.
- [nFPM](https://nfpm.goreleaser.com/docs/) currently supports Debian, RPM and Arch-family packages (and other formats) from one configuration.

The likely split is **cargo-packager for the Windows installer and perhaps AppImage, nFPM for deb/rpm**, but this is not locked in. Prefer the smallest configuration that produces conventional packages. Do not reshape OSP's architecture around a packager.

## D0 — compatibility and packaging spike

Before writing production installer code:

1. Build `osp` for `x86_64-pc-windows-msvc` in CI and record every compile/runtime blocker.
2. Audit Linux assumptions in `osp-cli`, `osp-node` and `osp-tor`: systemd, Unix file permissions, executable discovery, paths, process spawning, signals and tests that use Unix-only APIs.
3. Prove one throwaway Windows build can start a node, run Tor, find ffmpeg/ffprobe, post a photo/video and pair a phone. It does not need an installer yet.
4. Decide how Windows receives Tor, ffmpeg/ffprobe and HEIC support. A public consumer installer must not say "install these three developer tools yourself." Pin versions, preserve upstream licence notices and record provenance/checksums.
5. Decide the **Windows service identity, node-home location and ACL model**. The node must start at boot before login, but its identity/data cannot live in `Program Files`. This is security/ownership-sensitive; write an ADR if the choice changes existing semantics.
6. Decide the first-run setup surface. A package manager's post-install script must not create the identity. If a native package alone cannot preserve the P11 no-terminal journey, add a small first-run launcher/setup UI (for example, an OSP-owned local setup flow) rather than falling back to developer instructions.
7. Build one toy `.exe`, one `.deb` and one `.rpm` with candidate tooling. Choose tools only after seeing installation, upgrade and uninstall behaviour.

**D0 gate:** OSP runs on Windows from a clean checkout/build; the service/data/dependency decisions are written down; package tooling is chosen; no Product Alpha behaviour is changed to satisfy the packager.

## D1 — one install layout and version contract

Create one packaging description of what belongs to the application versus the node home.

The contract should cover:

- executable(s), helper binaries and third-party runtime tools;
- service definitions/registration;
- licence and third-party notices;
- icons/desktop metadata where useful;
- version exposed by `osp --version` and `osp status`;
- node-home discovery and `OSP_HOME` overrides;
- upgrade, rollback and uninstall behaviour.

Linux's current node home remains `~/.local/share/osp` unless a separate migration decision changes it. Native Linux packages should normally place `osp` in `/usr/bin` (or the distro-standard equivalent), while the node home remains owned by the person.

On Windows, the exact node-home path is a D0 decision. Whatever path is chosen, **the installer must be replaceable without touching it**.

**D1 gate:** installing a newer program over an existing test node preserves the identity, config, database and media byte-for-byte except for intentional application migrations.

## D2 — Windows x86_64 installer

The Windows installer is a product installer, not merely a ZIP extractor.

It must:

- install OSP under the normal per-machine program location;
- install or bundle the runtime dependencies chosen in D0;
- register an always-on service that starts at boot and restarts after failure;
- apply least-privilege filesystem ACLs to the node home and service files;
- expose a normal first-run/setup entry point;
- show the installed version in Windows' installed-apps list;
- support unattended install/uninstall for CI;
- upgrade in place;
- uninstall program/service files while **preserving node data by default**;
- never open an inbound firewall rule unless a measured requirement says one is needed.

For internal smoke builds, unsigned packages are acceptable. Before the website presents Windows as a normal public download, establish code signing for the installer (and executable where appropriate) or explicitly document the warning users will see. Signing credentials never enter the repository.

**D2 gate:** on a clean Windows VM, an ordinary install can create/recover a node, start it at boot before login, pair a phone, post media, survive a reboot, upgrade to the next test build and uninstall/reinstall without losing the node.

## D3 — Debian/Ubuntu packages, x86_64 and arm64

Produce native `.deb` packages for both architectures.

The package should:

- install the OSP executable and documentation in distro-standard locations;
- declare compatible runtime dependencies instead of downloading them in maintainer scripts where practical;
- keep node data in the person's node home;
- preserve the current systemd-user-service + lingering behaviour unless an accepted ADR changes it;
- never run `osp init` as root from `postinst`;
- support upgrade without resetting an existing node;
- leave node data behind on normal package removal and make destructive purge behaviour explicit.

Test at least the currently supported Debian/Ubuntu baseline and one current release, plus a 64-bit Raspberry Pi OS/arm64 path. Package dependency names may differ by distro; encode those differences rather than weakening the media requirements.

**D3 gate:** fresh install, reboot-before-login, phone pairing, media post, upgrade and remove/reinstall all pass on amd64 and arm64.

## D4 — RPM and generic Linux

Once D2/D3 are stable:

- add RPM packages for x86_64 and arm64;
- test on at least one Fedora-family current release and one long-lived RHEL-compatible release if practical;
- retain a generic Linux artifact for unsupported distros.

An AppImage is useful as a portable executable, but it is **not by itself the service/install story** for an always-on node. If AppImage remains the generic fallback, provide a stable install location and explicit service integration, or keep the existing archive + installer flow for generic Linux until that is solved.

**D4 gate:** an RPM install has the same data/service/update semantics as the deb; the generic fallback clearly states what it does and does not automate.

## D5 — release automation, provenance and signing

Extend the existing [release workflow](../.github/workflows/release.yml), which already builds Linux x86_64/arm64 archives, checks the required glibc version, makes checksums and creates a draft GitHub release.

A release tag should eventually produce, at minimum:

```text
OSP-Setup-<version>-x64.exe
open-social-p2p_<version>_amd64.deb
open-social-p2p_<version>_arm64.deb
open-social-p2p-<version>.x86_64.rpm
open-social-p2p-<version>.aarch64.rpm
osp-linux-x86_64.tar.gz
osp-linux-aarch64.tar.gz
SHA256SUMS
SBOM(s)
```

The pipeline order should be:

1. tests;
2. native builds;
3. package assembly;
4. clean-machine install/upgrade smoke tests;
5. signing/attestation;
6. checksum/SBOM generation;
7. **draft** GitHub release;
8. human review and publication.

Use GitHub Actions artifacts only as handoff between jobs; the public downloadable artifacts belong on the GitHub Release. Add build provenance/artifact attestations where practical. Release jobs get the minimum permissions they need.

Do not remove `scripts/install.sh` or the raw tarballs until the new packages have passed at least one real release upgrade cycle.

**D5 gate:** one tag produces every expected artifact from source, with no manual rebuilding on a maintainer laptop; all artifacts identify the same OSP version and clean-install tests passed before the draft can be published.

## D6 — osp2p.com download flow

Only after artifact names and release automation are stable, update the separate `osp2p-site` repository.

The download page should make the common choice obvious:

- **Windows:** Download for Windows.
- **Linux:** Ubuntu/Debian, Fedora/RPM, Generic Linux.
- **Other downloads:** arm64 and advanced/manual artifacts.

OS detection may highlight a likely choice, but it must never hide the other downloads. The site should link to versioned GitHub Release artifacts or a small explicit release manifest rather than scrape the Releases HTML.

Keep the website lightweight: the large binaries stay on GitHub Releases unless there is a later reason to mirror them.

**D6 gate:** from a fresh browser, a Windows or Linux user can identify the right installer without knowing CPU/distro terminology beyond what is unavoidable, and every displayed link points to the release version the page claims.

## D7 — updates

Do this **after** ordinary install/upgrade packages are reliable.

Requirements:

- the node can learn that an update exists without a central account;
- update metadata is signed and its trust root is defined in an ADR;
- the updater verifies the artifact before replacing executable files;
- node data is never part of the application update payload;
- the service stops cleanly, updates, restarts and reports the new version;
- failed updates leave a known rollback path;
- updates may be deferred by the owner;
- security updates can be made prominent without silently changing identity/privacy settings.

Do not treat `SHA256SUMS` fetched from the same release location as sufficient authentication for automatic updates.

**D7 gate:** a deliberately corrupted or wrongly signed update is refused; a killed/interrupted update leaves either the old or new executable runnable and the same node data intact.

## D8 — later package channels

Only after the direct installers are boring and reliable:

- Windows arm64;
- Arch package/AUR workflow;
- apt and rpm repositories for one-command managed updates;
- Microsoft Store/MSIX if it does not weaken the always-on service model;
- Flatpak/Snap only with a tested service/sandbox design;
- package-manager/community channels requested by real users.

Store distribution is a convenience layer, not the source of OSP identity or update authority.

## Clean-machine acceptance matrix

For every supported package format, record a testing file under `docs/testing/` with the actual commands/screens and result.

Minimum scenarios:

| Scenario | Required result |
| --- | --- |
| Fresh install | Program and dependencies install without developer tooling |
| First run | Person can create or recover a node without root-owned identity files |
| Boot before login | Node becomes reachable after reboot before interactive login |
| Phone pairing | Android/iPhone client pairs and sees node health |
| Media | Photo and video posting exercise ffmpeg + HEIC path where available |
| Network | Tor/control endpoint and media path work from a normal home connection |
| Upgrade | Old node data opens under new program without identity reset |
| Interrupted upgrade | Recoverable; node home intact |
| Uninstall | Service/program removed, node data preserved by default |
| Reinstall | Existing node is detected and reused |
| Wrong architecture | Installer/package fails clearly rather than partially installing |
| Low privileges | Runtime does not require administrator/root after installation |
| Version | CLI/status/package metadata/release tag agree |

For Windows, add a test that no interactive user has logged in after reboot. For Linux, keep x86_64 and arm64 as separate clean-machine results.

## Handoff instructions for the future agent

When the Product Alpha roadmap is complete:

1. Read ADRs [0005](decisions/0005-media-profile-v1.md), [0006](decisions/0006-desktop-first-always-on-node.md), [0008](decisions/0008-device-pairing-and-client-protocol.md), [0009](decisions/0009-identity-backup-and-recovery.md) and [0010](decisions/0010-signing-keys-and-revocation.md).
2. Read `docs/install.md`, `scripts/install.sh`, `.github/workflows/release.yml` and the latest P11/P12 testing record.
3. Start with **D0 only**. Do not immediately add five package formats.
4. Open ADRs for the Windows service/data ownership model and automatic-update trust model if those decisions are not already settled elsewhere.
5. Get the first vertical slice — Windows x64 `.exe`, Debian amd64 `.deb`, Debian arm64 `.deb` — through the clean-machine gates.
6. Keep the old Linux installer available until the replacement survives an actual upgrade cycle.
7. Only then add RPM/AppImage, website auto-selection and automatic updates.

The success condition is not "CI produced files." It is that someone who did not build OSP can install a home node in the normal way for their operating system, reboot the computer, use the phone app, update later, and never have to understand where the Rust binary lives.
