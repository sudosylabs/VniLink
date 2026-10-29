//! Which GPU encodes the capture.
//!
//! Portal and X11 pictures arrive in shared memory (DMA-BUF capture is off,
//! see [`crate::pipewire`]). The compositor, or the X server, draws every
//! output on its primary GPU and copies the picture out from there; GNOME's
//! mutter, KDE's KWin and wlroots compositors all render on one primary GPU
//! and copy to outputs on another. An encoder on that GPU takes the picture
//! the shortest way. One on another GPU, such as a hybrid laptop's discrete
//! GPU while the panel is on the integrated one, receives every picture
//! over PCIe. Which output is captured does not change this while capture
//! is shared memory; once DMA-BUF capture is on, the buffer's device names
//! the GPU instead.
//!
//! Encoders are therefore tried GPU by GPU ([`encoders`]): first the GPU the
//! compositor takes as primary, by the rule the compositors share — GNOME's
//! `mutter-device-preferred-primary` udev tag where a machine sets one, else
//! the boot VGA device the firmware showed its screen on — then GPUs driving
//! a connected display, then the rest. Ties go by PCI address, then render
//! node, so a machine with several GPUs always makes the same choice. On an
//! NVIDIA GPU NVENC is tried before VA-API; elsewhere VA-API alone.
//!
//! The rule reads sysfs and udev's database only. Measured on one hybrid
//! Intel + NVIDIA laptop under GNOME; KWin, wlroots, AMD and two discrete
//! GPUs follow from the same rule without having been run.

use std::path::{Path, PathBuf};

/// The PCI vendor ID of NVIDIA, whose GPUs offer NVENC.
pub(crate) const NVIDIA: u16 = 0x10de;

/// The udev tag GNOME reads to take a GPU as primary.
const PREFERRED_PRIMARY_TAG: &str = "mutter-device-preferred-primary";

/// One GPU with a DRM render node, as sysfs describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Gpu {
    /// The render node, `/dev/dri/renderD128`.
    pub(crate) render_node: PathBuf,
    /// The PCI address, `0000:01:00.0`; `None` for a device not on PCI,
    /// such as a system-on-chip GPU, or when sysfs is unreadable.
    pub(crate) pci_address: Option<String>,
    /// The PCI vendor ID, when sysfs names one.
    pub(crate) vendor: Option<u16>,
    /// The firmware showed its screen on this GPU.
    pub(crate) boot_vga: bool,
    /// udev tags this GPU as GNOME's primary.
    pub(crate) preferred_primary: bool,
    /// A display is connected to one of this GPU's outputs.
    pub(crate) drives_display: bool,
}

/// One encoder to try, in [`encoders`] order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EncoderChoice {
    /// NVENC on the NVIDIA GPU at this PCI address; `None` takes CUDA's
    /// first device, for a machine whose GPUs sysfs does not describe.
    Nvenc { pci_address: Option<String> },
    /// VA-API on this render node.
    Vaapi { render_node: PathBuf },
}

/// This machine's GPUs, in render node order.
#[cfg(target_os = "linux")]
pub(crate) fn inventory() -> Vec<Gpu> {
    describe(
        Path::new("/sys/class/drm"),
        Path::new("/run/udev/data"),
        &crate::vaapi::render_nodes(),
    )
}

/// This machine's render nodes in [`ranked`] order.
#[cfg(target_os = "linux")]
pub(crate) fn ranked_render_nodes() -> Vec<PathBuf> {
    ranked(&inventory())
        .into_iter()
        .map(|gpu| gpu.render_node)
        .collect()
}

/// `gpus` in the order encoders and decoders try them: the GPU the
/// compositor composites on first.
pub(crate) fn ranked(gpus: &[Gpu]) -> Vec<Gpu> {
    let primary = compositing(gpus);
    let mut ranked = gpus.to_vec();
    ranked.sort_by(|a, b| {
        let rank = |gpu: &Gpu| {
            (
                Some(gpu) != primary,
                !gpu.drives_display,
                gpu.pci_address.is_none(),
            )
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.pci_address.cmp(&b.pci_address))
            .then_with(|| a.render_node.cmp(&b.render_node))
    });
    ranked
}

/// The GPU the compositor takes as primary, when one is marked: the udev
/// tag wins over boot VGA, and either must name exactly one GPU.
pub(crate) fn compositing(gpus: &[Gpu]) -> Option<&Gpu> {
    let only = |marked: fn(&Gpu) -> bool| {
        let mut found = gpus.iter().filter(|gpu| marked(gpu));
        match (found.next(), found.next()) {
            (Some(gpu), None) => Some(gpu),
            _ => None,
        }
    };
    only(|gpu| gpu.preferred_primary).or_else(|| only(|gpu| gpu.boot_vga))
}

/// The encoders to try for `gpus`, best first.
pub(crate) fn encoders(gpus: &[Gpu]) -> Vec<EncoderChoice> {
    let mut choices = Vec::new();
    for gpu in ranked(gpus) {
        if gpu.vendor == Some(NVIDIA) {
            choices.push(EncoderChoice::Nvenc {
                pci_address: gpu.pci_address.clone(),
            });
        }
        choices.push(EncoderChoice::Vaapi {
            render_node: gpu.render_node,
        });
    }
    // Without vendors there is no telling which GPU is NVIDIA's: CUDA's
    // first device comes after every VA-API node, as a last resort.
    if gpus.iter().all(|gpu| gpu.vendor.is_none()) {
        choices.push(EncoderChoice::Nvenc { pci_address: None });
    }
    choices
}

/// Describes each of `render_nodes` from the sysfs DRM class at `drm` and
/// udev's database at `udev`. A node sysfs does not describe keeps only its
/// path.
pub(crate) fn describe(drm: &Path, udev: &Path, render_nodes: &[PathBuf]) -> Vec<Gpu> {
    render_nodes
        .iter()
        .map(|render_node| {
            let mut gpu = Gpu {
                render_node: render_node.clone(),
                ..Gpu::default()
            };
            let Some(name) = render_node.file_name() else {
                return gpu;
            };
            let device = drm.join(name).join("device");
            gpu.pci_address = std::fs::canonicalize(&device)
                .ok()
                .and_then(|path| path.file_name()?.to_str().map(str::to_owned))
                .filter(|name| is_pci_address(name));
            gpu.vendor = read_trimmed(&device.join("vendor"))
                .and_then(|id| u16::from_str_radix(id.trim_start_matches("0x"), 16).ok());
            gpu.boot_vga = read_trimmed(&device.join("boot_vga")).as_deref() == Some("1");
            for card in cards(&device.join("drm")) {
                let card_dir = device.join("drm").join(&card);
                gpu.drives_display |= connected(&card_dir, &card);
                gpu.preferred_primary |= read_trimmed(&card_dir.join("dev")).is_some_and(|dev| {
                    tagged(&udev.join(format!("c{dev}")), PREFERRED_PRIMARY_TAG)
                });
            }
            gpu
        })
        .collect()
}

/// `0000:01:00.0`: domain, bus, device and function.
fn is_pci_address(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 12
        && bytes[4] == b':'
        && bytes[7] == b':'
        && bytes[10] == b'.'
        && name
            .bytes()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7 | 10) || byte.is_ascii_hexdigit())
}

/// The `cardN` entries of a device's `drm` directory.
fn cards(drm: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(drm) else {
        return Vec::new();
    };
    let mut cards: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| {
            name.strip_prefix("card")
                .is_some_and(|index| !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()))
        })
        .collect();
    cards.sort();
    cards
}

/// Whether one of `card`'s connectors, `card0-eDP-1`, reports a display.
fn connected(card_dir: &Path, card: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(card_dir) else {
        return false;
    };
    let prefix = format!("{card}-");
    entries.flatten().any(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(&prefix))
            && read_trimmed(&entry.path().join("status")).as_deref() == Some("connected")
    })
}

/// Whether udev's record of a device lists `tag` (`G:` lines, and `Q:` for
/// current tags in newer udev).
fn tagged(record: &Path, tag: &str) -> bool {
    std::fs::read_to_string(record).is_ok_and(|text| {
        text.lines().any(|line| {
            line.strip_prefix("G:")
                .or_else(|| line.strip_prefix("Q:"))
                .is_some_and(|value| value == tag)
        })
    })
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
}
