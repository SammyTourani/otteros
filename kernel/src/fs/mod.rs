//! Filesystem-shaped things the kernel needs before a real VFS exists
//! (that's M3): today, just the boot-time ustar initramfs (brief M2-T3).

pub mod initramfs;
