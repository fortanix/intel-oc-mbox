/* Copyright (c) Fortanix, Inc.
 *
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

use std::io::{self, Result};
use std::process::Command;

mod msr {
    use std::fs::{File, OpenOptions};
    use std::io::{prelude::*, SeekFrom, Result};

    pub const OC_MBOX: u64 = 0x150;
    pub const FLEX_RATIO: u64 = 0x194;
    pub const FLEX_RATIO_OC_LOCK: u64 = 1 << 20;

    pub struct Msr {
        dev: File,
        num: u64,
    }

    impl Msr {
        pub fn with_cpu(cpu: usize, num: u64) -> Result<Self> {
            let f = format!("/dev/cpu/{}/msr", cpu);
            Ok(Msr {
                dev: OpenOptions::new().read(true).write(true).open(f)?,
                num
            })
        }

        fn seek(&self) -> Result<()> {
            (&self.dev).seek(SeekFrom::Start(self.num))?;
            Ok(())
        }

        pub fn read(&self) -> Result<u64> {
            self.seek()?;
            let mut buf = [0u8; 8];
            (&self.dev).read_exact(&mut buf)?;
            Ok(u64::from_le_bytes(buf))
        }

        pub fn write(&self, value: u64) -> Result<()> {
            self.seek()?;
            (&self.dev).write_all(&value.to_le_bytes())
        }
    }
}

mod oc_mbox {
    use std::io::{self, Result};

    use crate::msr::{self, Msr};

    #[repr(u8)]
    #[derive(Clone, Copy)]
    pub enum Domain {
        IaCore = 0,
        GtSlices,
        CboLlcRing,
        GtUnslice,
        SystemAgent,
    }


    pub struct OcMbox {
        msr: Msr
    }

    impl OcMbox {
        pub fn with_cpu(cpu: usize) -> Result<Self> {
            Ok(OcMbox {
                msr: Msr::with_cpu(cpu, msr::OC_MBOX)?
            })
        }

        fn poll_result(&self) -> Result<Result<u32>> {
            loop {
                let val = self.msr.read()?;
                let r = val >> 63;
                let c = (val >> 32) as u8;
                let d = val as u32;
                if r == 0 {
                    let errinfo = match c {
                        0 => return Ok(Ok(d)),
                        1 => &"Overclocking is locked" as &dyn std::fmt::Display,
                        0x1f => &"Unrecognized command" as _,
                        _ => &c as _
                    };
                    return Ok(Err(io::Error::new(io::ErrorKind::Other, format!("Mailbox returned error: {}", errinfo))))
                }
            }
        }

        pub fn cmd(&self, command: u8, param1: u8, param2: u8, data: u32) -> Result<u32> {
            // wait until mailbox is available
            // WARNING: racy
            let _ = self.poll_result()?;

            // send mailbox command
            let msg = (data as u64) |
                ((command as u64) << 32) |
                ((param1 as u64) << 40) |
                ((param2 as u64) << 48) |
                (1u64 << 63);
            self.msr.write(msg)?;

            // wait for mailbox completion
            self.poll_result()?
        }
    }

    pub const CMD_VF_OVERRIDE_READ: u8 = 0x10;
}

fn main() -> Result<()> {
    let status = Command::new("modprobe").arg("msr").status()?;
    if !status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, format!("modprobe exited with status: {}", status)))
    }

    let flex_ratio = msr::Msr::with_cpu(0, msr::FLEX_RATIO)?.read()?;
    println!("Overclocking lock: {}", (flex_ratio & msr::FLEX_RATIO_OC_LOCK) != 0);

    let ocmbox = oc_mbox::OcMbox::with_cpu(0)?;
    
    use oc_mbox::Domain::*;
    let domains = [IaCore, GtSlices, CboLlcRing, GtUnslice, SystemAgent];
    for &domain in &domains {
        println!("domain {}: {:08x}", domain as u8, ocmbox.cmd(oc_mbox::CMD_VF_OVERRIDE_READ, domain as _, 0, 0)?);
    }
    
    Ok(())
}
