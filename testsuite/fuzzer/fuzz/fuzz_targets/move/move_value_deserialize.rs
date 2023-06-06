// Copyright © Aptos Foundation
// SPDX-License-Identifier: Apache-2.0

#![no_main]
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use move_core_types::value::{MoveTypeLayout, MoveValue};

mod utils;

#[derive(Arbitrary, Debug)]
struct FuzzData {
    data: Vec<u8>,
    layout: MoveTypeLayout,
}

fuzz_target!(|fuzz_data: FuzzData| {
    if fuzz_data.data.len() <= 0  || !utils::is_valid_layout(&fuzz_data.layout) {
        return;
    }
    let _ = MoveValue::simple_deserialize(&fuzz_data.data, &fuzz_data.layout);
});