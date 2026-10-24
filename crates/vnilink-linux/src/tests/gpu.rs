use std::path::{Path, PathBuf};

use crate::gpu::{compositing, describe, encoders, ranked, EncoderChoice, Gpu, NVIDIA};

const INTEL: u16 = 0x8086;
const AMD: u16 = 0x1002;

fn gpu(node: u32, pci: &str, vendor: u16) -> Gpu {
    Gpu {
        render_node: PathBuf::from(format!("/dev/dri/renderD{node}")),
        pci_address: Some(pci.into()),
        vendor: Some(vendor),
        ..Gpu::default()
    }
}

fn nodes(gpus: &[Gpu]) -> Vec<String> {
    ranked(gpus)
        .iter()
        .map(|gpu| gpu.render_node.display().to_string())
        .collect()
}

fn vaapi(node: u32) -> EncoderChoice {
    EncoderChoice::Vaapi {
        render_node: PathBuf::from(format!("/dev/dri/renderD{node}")),
    }
}

fn nvenc(pci: &str) -> EncoderChoice {
    EncoderChoice::Nvenc {
        pci_address: Some(pci.into()),
    }
}

/// An integrated Intel GPU at 00:02.0 and a discrete NVIDIA one at 01:00.0.
fn hybrid() -> (Gpu, Gpu) {
    (
        gpu(128, "0000:00:02.0", INTEL),
        gpu(129, "0000:01:00.0", NVIDIA),
    )
}

#[test]
fn a_single_gpu_is_the_only_choice() {
    let mut only = gpu(128, "0000:00:02.0", INTEL);
    only.boot_vga = true;
    only.drives_display = true;
    assert_eq!(encoders(&[only.clone()]), [vaapi(128)]);
    assert_eq!(compositing(&[only.clone()]), Some(&only));
}

#[test]
fn a_hybrid_laptop_with_its_panel_on_the_integrated_gpu_encodes_there_first() {
    let (mut intel, nvidia) = hybrid();
    intel.boot_vga = true;
    intel.drives_display = true;
    // The order sysfs lists them in does not matter.
    for gpus in [
        [intel.clone(), nvidia.clone()],
        [nvidia.clone(), intel.clone()],
    ] {
        assert_eq!(
            encoders(&gpus),
            [vaapi(128), nvenc("0000:01:00.0"), vaapi(129)]
        );
    }
}

#[test]
fn a_monitor_on_the_discrete_gpu_leaves_the_compositor_on_the_integrated_one() {
    let (mut intel, mut nvidia) = hybrid();
    intel.boot_vga = true;
    intel.drives_display = true;
    nvidia.drives_display = true;
    assert_eq!(
        encoders(&[intel, nvidia]),
        [vaapi(128), nvenc("0000:01:00.0"), vaapi(129)]
    );
}

#[test]
fn a_hybrid_laptop_with_its_panel_on_the_discrete_gpu_encodes_there_first() {
    // A laptop switched to its discrete GPU: the firmware shows its screen
    // there, and the integrated GPU drives nothing.
    let (intel, mut nvidia) = hybrid();
    nvidia.boot_vga = true;
    nvidia.drives_display = true;
    assert_eq!(
        encoders(&[intel, nvidia]),
        [nvenc("0000:01:00.0"), vaapi(129), vaapi(128)]
    );
}

#[test]
fn gnomes_preferred_primary_tag_wins_over_boot_vga() {
    let (mut intel, mut nvidia) = hybrid();
    intel.boot_vga = true;
    intel.drives_display = true;
    nvidia.preferred_primary = true;
    assert_eq!(compositing(&[intel.clone(), nvidia.clone()]), Some(&nvidia));
    assert_eq!(
        nodes(&[intel, nvidia]),
        ["/dev/dri/renderD129", "/dev/dri/renderD128"]
    );
}

#[test]
fn two_discrete_gpus_follow_boot_vga() {
    let mut nvidia = gpu(128, "0000:01:00.0", NVIDIA);
    let mut amd = gpu(129, "0000:02:00.0", AMD);
    nvidia.drives_display = true;
    amd.boot_vga = true;
    amd.drives_display = true;
    assert_eq!(
        encoders(&[nvidia, amd]),
        [vaapi(129), nvenc("0000:01:00.0"), vaapi(128)]
    );
}

#[test]
fn two_discrete_gpus_without_a_primary_go_by_display_then_pci_address() {
    let first = gpu(129, "0000:01:00.0", NVIDIA);
    let mut second = gpu(128, "0000:02:00.0", NVIDIA);
    assert_eq!(compositing(&[first.clone(), second.clone()]), None);
    assert_eq!(
        nodes(&[second.clone(), first.clone()]),
        ["/dev/dri/renderD129", "/dev/dri/renderD128"]
    );
    second.drives_display = true;
    assert_eq!(
        nodes(&[first, second]),
        ["/dev/dri/renderD128", "/dev/dri/renderD129"]
    );
}

#[test]
fn a_mark_on_two_gpus_names_no_primary() {
    let (mut intel, mut nvidia) = hybrid();
    intel.boot_vga = true;
    nvidia.boot_vga = true;
    assert_eq!(compositing(&[intel, nvidia]), None);
}

#[test]
fn no_gpu_leaves_only_cudas_first_device_to_try() {
    // Without a render node there is nothing for VA-API; NVENC still gets
    // its chance, and a machine without it reports no hardware encoder.
    assert_eq!(encoders(&[]), [EncoderChoice::Nvenc { pci_address: None }]);
}

#[test]
fn render_nodes_sysfs_does_not_describe_are_tried_in_name_order() {
    let blank = |node: u32| Gpu {
        render_node: PathBuf::from(format!("/dev/dri/renderD{node}")),
        ..Gpu::default()
    };
    assert_eq!(
        encoders(&[blank(129), blank(128)]),
        [
            vaapi(128),
            vaapi(129),
            EncoderChoice::Nvenc { pci_address: None }
        ]
    );
}

/// A sysfs DRM class and udev database under a fresh temporary directory.
struct FakeSystem {
    root: PathBuf,
}

impl FakeSystem {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("vnilink-gpu-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("class/drm")).unwrap();
        std::fs::create_dir_all(root.join("udev")).unwrap();
        Self { root }
    }

    fn drm(&self) -> PathBuf {
        self.root.join("class/drm")
    }

    fn udev(&self) -> PathBuf {
        self.root.join("udev")
    }

    /// A PCI GPU with one card, one render node and the given connectors.
    fn gpu(&self, pci: &str, vendor: &str, card: u32, render: u32, connectors: &[(&str, &str)]) {
        let device = self.root.join("devices/pci0000:00").join(pci);
        let card_dir = device.join(format!("drm/card{card}"));
        std::fs::create_dir_all(&card_dir).unwrap();
        std::fs::create_dir_all(device.join(format!("drm/renderD{render}"))).unwrap();
        write(&device.join("vendor"), &format!("{vendor}\n"));
        write(&card_dir.join("dev"), &format!("226:{card}\n"));
        for (connector, status) in connectors {
            let dir = card_dir.join(format!("card{card}-{connector}"));
            std::fs::create_dir_all(&dir).unwrap();
            write(&dir.join("status"), &format!("{status}\n"));
        }
        let class = self.drm().join(format!("renderD{render}"));
        std::fs::create_dir_all(&class).unwrap();
        std::os::unix::fs::symlink(&device, class.join("device")).unwrap();
    }

    fn device(&self, pci: &str) -> PathBuf {
        self.root.join("devices/pci0000:00").join(pci)
    }
}

impl Drop for FakeSystem {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

#[test]
fn sysfs_describes_a_hybrid_laptop() {
    let system = FakeSystem::new("hybrid");
    system.gpu(
        "0000:00:02.0",
        "0x8086",
        1,
        128,
        &[("eDP-1", "connected"), ("HDMI-A-1", "disconnected")],
    );
    system.gpu(
        "0000:01:00.0",
        "0x10de",
        0,
        129,
        &[("DP-1", "disconnected")],
    );
    write(&system.device("0000:00:02.0").join("boot_vga"), "1\n");
    write(&system.device("0000:01:00.0").join("boot_vga"), "0\n");
    let nodes = [
        PathBuf::from("/dev/dri/renderD128"),
        PathBuf::from("/dev/dri/renderD129"),
        PathBuf::from("/dev/dri/renderD130"),
    ];
    let gpus = describe(&system.drm(), &system.udev(), &nodes);
    assert_eq!(
        gpus[0],
        Gpu {
            render_node: nodes[0].clone(),
            pci_address: Some("0000:00:02.0".into()),
            vendor: Some(INTEL),
            boot_vga: true,
            preferred_primary: false,
            drives_display: true,
        }
    );
    assert_eq!(gpus[1].vendor, Some(NVIDIA));
    assert!(!gpus[1].boot_vga && !gpus[1].drives_display);
    // A node sysfs does not know keeps only its path.
    assert_eq!(
        gpus[2],
        Gpu {
            render_node: nodes[2].clone(),
            ..Gpu::default()
        }
    );
    assert_eq!(
        encoders(&gpus[..2]),
        [vaapi(128), nvenc("0000:01:00.0"), vaapi(129)]
    );
}

#[test]
fn udev_tags_the_preferred_primary_on_its_card() {
    let system = FakeSystem::new("tagged");
    system.gpu("0000:00:02.0", "0x8086", 1, 128, &[("eDP-1", "connected")]);
    system.gpu("0000:01:00.0", "0x10de", 0, 129, &[]);
    write(
        &system.udev().join("c226:0"),
        "E:ID_PATH=pci-0000:01:00.0\nG:seat\nQ:mutter-device-preferred-primary\n",
    );
    let gpus = describe(
        &system.drm(),
        &system.udev(),
        &[
            PathBuf::from("/dev/dri/renderD128"),
            PathBuf::from("/dev/dri/renderD129"),
        ],
    );
    assert!(!gpus[0].preferred_primary);
    assert!(gpus[1].preferred_primary);
    assert_eq!(compositing(&gpus), Some(&gpus[1]));
}
