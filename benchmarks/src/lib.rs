// Copyright (c) 2020-present, UMD Database Group.
//
// This program is free software: you can use, redistribute, and/or modify
// it under the terms of the GNU Affero General Public License, version 3
// or later ("AGPL"), as published by the Free Software Foundation.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or
// FITNESS FOR A PARTICULAR PURPOSE.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program. If not, see <http://www.gnu.org/licenses/>.

#[path = "./nexmark/main.rs"]
pub mod nexmark;
pub use nexmark::{nexmark_benchmark, NexmarkBenchmarkOpt};

#[path = "./ysb/main.rs"]
pub mod ysb;
pub use ysb::{ysb_benchmark, YSBBenchmarkOpt};

#[path = "./arch/main.rs"]
pub mod arch;
pub use arch::{arch_benchmark, ArchBenchmarkOpt};

pub mod rainbow;
pub use rainbow::{rainbow_println, rainbow_string};
