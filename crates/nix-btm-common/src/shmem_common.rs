use std::{
    io::{Error, ErrorKind, Result},
    marker::PhantomData,
    os::fd::{AsFd, OwnedFd},
};

use memmap2::{MmapMut, MmapOptions};
use rustix::fs::{MemfdFlags, ftruncate, memfd_create};
