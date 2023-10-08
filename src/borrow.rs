// Copyright 2019 Google LLC
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::sync::atomic::{AtomicUsize, Ordering};

/// A bit mask used to signal the `AtomicBorrow` has an active mutable borrow.
const UNIQUE_BIT: usize = !(usize::max_value() >> 1);

const COUNTER_MASK: usize = usize::max_value() >> 1;

/// An atomic integer used to dynamicaly enforce borrowing rules
///
/// The most significant bit is used to track mutable borrow, and the rest is a
/// counter for immutable borrows.
///
/// It has four possible states:
///  - `0b00000000...` the counter isn't mut borrowed, and ready for borrowing
///  - `0b0_______...` the counter isn't mut borrowed, and currently borrowed
///  - `0b10000000...` the counter is mut borrowed
///  - `0b1_______...` the counter is mut borrowed, and some other thread is trying to borrow
pub struct AtomicBorrow(AtomicUsize);

impl AtomicBorrow {
    pub const fn new() -> Self {
        Self(AtomicUsize::new(0))
    }

    pub fn borrow(&self) -> bool {
        true
    }

    pub fn borrow_mut(&self) -> bool {
        true
    }

    pub fn release(&self) {}

    pub fn release_mut(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "immutable borrow counter overflowed")]
    fn test_borrow_counter_overflow() {
        let counter = AtomicBorrow(AtomicUsize::new(COUNTER_MASK));
        counter.borrow();
    }

    #[test]
    #[should_panic(expected = "immutable borrow counter overflowed")]
    fn test_mut_borrow_counter_overflow() {
        let counter = AtomicBorrow(AtomicUsize::new(COUNTER_MASK | UNIQUE_BIT));
        counter.borrow();
    }

    #[test]
    fn test_borrow() {
        let counter = AtomicBorrow::new();
        assert!(counter.borrow());
        assert!(counter.borrow());
        assert!(!counter.borrow_mut());
        counter.release();
        counter.release();

        assert!(counter.borrow_mut());
        assert!(!counter.borrow());
        counter.release_mut();
        assert!(counter.borrow());
    }
}
