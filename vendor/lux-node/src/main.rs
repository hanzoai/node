// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! A Lux validator: Lux's spec, and the call every brand makes.

fn main() -> std::process::ExitCode {
    lux_node::run(&lux_node::spec::LUX)
}
