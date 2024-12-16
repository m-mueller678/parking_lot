// Copyright 2016 Amanieu d'Antras
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use hermit_abi::{
    futex_wait, futex_wake, time_t, timespec, EAGAIN, EINVAL, ETIMEDOUT, FUTEX_RELATIVE_TIMEOUT,
};
use std::ops::ControlFlow;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;
use std::{ptr, thread};

const UNPARKED: u32 = 0;
const PARKED: u32 = 1;

// Helper type for putting a thread to sleep until some other thread wakes it up
pub struct ThreadParker {
    futex: AtomicU32,
}

impl super::ThreadParkerT for ThreadParker {
    type UnparkHandle = UnparkHandle;

    const IS_CHEAP_TO_CONSTRUCT: bool = true;

    #[inline]
    fn new() -> ThreadParker {
        ThreadParker {
            futex: AtomicU32::new(UNPARKED),
        }
    }

    #[inline]
    unsafe fn prepare_park(&self) {
        self.futex.store(PARKED, Relaxed);
    }

    #[inline]
    unsafe fn timed_out(&self) -> bool {
        self.futex.load(Relaxed) != UNPARKED
    }

    #[inline]
    unsafe fn park(&self) {
        while self.futex.load(Acquire) != UNPARKED {
            match self.futex_wait_relative(None) {
                ControlFlow::Break(_) => return,
                ControlFlow::Continue(()) => continue,
            }
        }
    }

    #[inline]
    unsafe fn park_until(&self, timeout: Instant) -> bool {
        while self.futex.load(Acquire) != UNPARKED {
            let now = Instant::now();
            if timeout <= now {
                return false;
            }
            let diff = timeout - now;
            if diff.as_secs() > time_t::MAX as u64 {
                // Timeout overflowed, just sleep indefinitely
                self.park();
                return true;
            }
            let ts = timespec {
                tv_sec: diff.as_secs() as time_t,
                tv_nsec: diff.subsec_nanos() as i32,
            };
            // ideally, we would specify an absolute timespec,
            // but it is currently not possible to extract one from Instant
            match self.futex_wait_relative(Some(ts)) {
                ControlFlow::Break(x) => return x,
                ControlFlow::Continue(()) => continue,
            }
        }
        true
    }

    #[inline]
    unsafe fn unpark_lock(&self) -> UnparkHandle {
        // We don't need to lock anything, just clear the state
        self.futex.store(UNPARKED, Release);

        UnparkHandle { futex: self.ptr() }
    }
}

impl ThreadParker {
    #[inline]
    fn futex_wait_relative(&self, ts: Option<timespec>) -> ControlFlow<bool, ()> {
        let r = unsafe {
            futex_wait(
                self.ptr(),
                PARKED,
                ts.as_ref()
                    .map(|x| x as *const timespec)
                    .unwrap_or(ptr::null()),
                FUTEX_RELATIVE_TIMEOUT,
            )
        };
        if r == 0 {
            return ControlFlow::Break(true);
        } else if r == -ETIMEDOUT {
            return ControlFlow::Break(false);
        } else if r != -EAGAIN {
            futex_return_unexpected(r);
        }
        ControlFlow::Continue(())
    }

    #[inline]
    fn ptr(&self) -> *mut u32 {
        &self.futex as *const AtomicU32 as *mut u32
    }
}

fn futex_return_unexpected(x: i32) {
    if cfg!(debug_assertions) {
        panic!("futex returned an unexpected value: {x}")
    }
}

pub struct UnparkHandle {
    futex: *mut u32,
}

impl super::UnparkHandleT for UnparkHandle {
    #[inline]
    unsafe fn unpark(self) {
        // The thread data may have been freed at this point, but the implementation of futex_wake
        // does not actually inspect the pointed data. It only uses the address as a key.
        let r = unsafe { futex_wake(self.futex, i32::MAX) };
        if r < 0 || r > 1 {
            futex_return_unexpected(r);
        }
    }
}

#[inline]
pub fn thread_yield() {
    thread::yield_now();
}
