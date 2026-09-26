//! GPUs and the frames that stay on them.
//!
//! A frame the compositor renders into a buffer on its own GPU (a DMA-BUF) never touches
//! the CPU: the encoder on that GPU reads it directly. [`GpuDevice`] says which GPU a
//! device number is (from sysfs), so frames and encoders can be matched up by device
//! rather than by vendor.

use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::PixelRect;

/// A DRM device (a GPU), as the kernel describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuDevice {
    /// The device number the compositor reported (a primary or render node).
    pub dev: u64,
    /// Its render node (`/dev/dri/renderD128`), for allocating buffers and matching
    /// encoders.
    pub render_node: Option<PathBuf>,
    /// PCI vendor id: 0x10de NVIDIA, 0x1002 AMD, 0x8086 Intel. None for non-PCI GPUs.
    pub vendor: Option<u16>,
    /// The kernel driver (`amdgpu`, `i915`, `xe`, `nvidia`, `nouveau`, …).
    pub driver: Option<String>,
    /// Where it sits on the bus (`0000:01:00.0`), telling apart two GPUs of a vendor.
    pub bus: Option<String>,
}

pub const VENDOR_NVIDIA: u16 = 0x10de;
pub const VENDOR_AMD: u16 = 0x1002;
pub const VENDOR_INTEL: u16 = 0x8086;

impl GpuDevice {
    /// Look up device number `dev` (`st_rdev` of a node in `/dev/dri`).
    pub fn from_dev(dev: u64) -> Self {
        Self::from_sysfs(
            dev,
            Path::new(&format!("/sys/dev/char/{}:{}", major(dev), minor(dev))),
        )
    }

    /// The device behind a node like `/dev/dri/renderD128`.
    pub fn from_node(node: &Path) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        Some(Self::from_dev(std::fs::metadata(node).ok()?.rdev()))
    }

    fn from_sysfs(dev: u64, node: &Path) -> Self {
        let device = node.join("device");
        let read = |name: &str| {
            std::fs::read_to_string(device.join(name))
                .ok()
                .map(|s| s.trim().to_string())
        };
        let render_node = std::fs::read_dir(device.join("drm"))
            .ok()
            .and_then(|dir| {
                dir.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .find(|name| name.starts_with("renderD"))
            })
            .map(|name| Path::new("/dev/dri").join(name));
        let link_name = |p: PathBuf| {
            std::fs::read_link(p)
                .ok()?
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        };
        Self {
            dev,
            render_node,
            vendor: read("vendor")
                .and_then(|v| u16::from_str_radix(v.trim_start_matches("0x"), 16).ok()),
            driver: link_name(device.join("driver")),
            bus: std::fs::canonicalize(&device)
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())),
        }
    }

    /// Whether `other` is the same GPU (primary and render nodes of one device are).
    pub fn same_as(&self, other: &GpuDevice) -> bool {
        match (&self.render_node, &other.render_node) {
            (Some(a), Some(b)) => a == b,
            _ => self.dev == other.dev,
        }
    }

    /// For logs: "nvidia 0000:01:00.0 (/dev/dri/renderD128)".
    pub fn describe(&self) -> String {
        let driver = self.driver.as_deref().unwrap_or("unknown driver");
        let node = self
            .render_node
            .as_ref()
            .map(|p| format!(" ({})", p.display()))
            .unwrap_or_default();
        format!("{driver} {}{node}", self.bus.as_deref().unwrap_or("?"))
    }
}

/// glibc's `gnu_dev_major`.
fn major(dev: u64) -> u64 {
    ((dev >> 8) & 0xfff) | ((dev >> 32) & 0xffff_f000)
}

/// glibc's `gnu_dev_minor`.
fn minor(dev: u64) -> u64 {
    (dev & 0xff) | ((dev >> 12) & 0xffff_ff00)
}

/// A DRM pixel format and the layouts (modifiers) it can have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmabufFormat {
    /// DRM fourcc, e.g. `XR24`.
    pub fourcc: u32,
    pub modifiers: Vec<u64>,
}

/// The linear (untiled) layout.
pub const MODIFIER_LINEAR: u64 = 0;
/// No explicit layout: the driver picks one both sides agree on.
pub const MODIFIER_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

/// What a frame source can render frames into instead of CPU memory.
#[derive(Debug, Clone)]
pub struct GpuOffer {
    pub device: GpuDevice,
    pub formats: Vec<DmabufFormat>,
}

/// A frame in GPU memory. Cloning is cheap; the source reuses the buffer once every clone
/// is dropped.
#[derive(Clone)]
pub struct Dmabuf {
    pub width: u32,
    pub height: u32,
    pub fourcc: u32,
    pub modifier: u64,
    pub planes: Vec<DmabufPlane>,
    /// The part to record, when not all of it (a region of an output).
    pub crop: Option<PixelRect>,
    /// Which of the source's buffers this is: the same one comes back every few frames.
    pub buffer: u64,
    pub lease: Arc<dyn Send + Sync>,
}

#[derive(Debug, Clone)]
pub struct DmabufPlane {
    pub fd: Arc<OwnedFd>,
    pub offset: u32,
    pub stride: u32,
}

impl std::fmt::Debug for Dmabuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dmabuf")
            .field("size", &(self.width, self.height))
            .field("fourcc", &fourcc_name(self.fourcc))
            .field("modifier", &format_args!("{:#x}", self.modifier))
            .field("planes", &self.planes.len())
            .field("crop", &self.crop)
            .field("buffer", &self.buffer)
            .finish()
    }
}

/// `XR24` for XRGB8888.
pub fn fourcc_name(fourcc: u32) -> String {
    fourcc
        .to_le_bytes()
        .iter()
        .map(|&b| if b.is_ascii_graphic() { b as char } else { '?' })
        .collect()
}

pub const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*code)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// glibc's `gnu_dev_makedev`.
    fn makedev(major: u64, minor: u64) -> u64 {
        ((major & 0xffff_f000) << 32)
            | ((major & 0xfff) << 8)
            | ((minor & 0xffff_ff00) << 12)
            | (minor & 0xff)
    }

    #[test]
    fn device_numbers_split_like_glibc() {
        assert_eq!(makedev(226, 128), 0xe280);
        for (maj, min) in [(226, 128), (0x1234, 0x56789)] {
            assert_eq!(
                (major(makedev(maj, min)), minor(makedev(maj, min))),
                (maj, min)
            );
        }
    }

    #[test]
    fn fourcc_names() {
        assert_eq!(fourcc_name(fourcc(b"XR24")), "XR24");
    }
}
