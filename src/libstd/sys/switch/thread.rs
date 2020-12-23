use crate::cmp;
use crate::ffi::CStr;
use crate::io;
use crate::mem;
use crate::ptr;
use crate::sys::os;
use crate::time::Duration;

use nnsdk::TimeSpan;

pub const STACK_GRANULARITY: usize = 0x1000;

#[cfg(not(target_os = "l4re"))]
pub const DEFAULT_MIN_STACK_SIZE: usize = 2 * 1024 * 1024;
#[cfg(target_os = "l4re")]
pub const DEFAULT_MIN_STACK_SIZE: usize = 1024 * 1024;
pub struct Thread {
    native: *mut nnsdk::os::ThreadType
}

unsafe impl Send for Thread {}
unsafe impl Sync for Thread {}

impl Thread {
    pub unsafe fn new(stack: usize, p: Box<dyn FnOnce()>) -> io::Result<Thread> {
        let p = Box::into_raw(box p);
        
        let _native: nnsdk::os::ThreadType = mem::zeroed();
        let mut native: *mut nnsdk::os::ThreadType = Box::into_raw(Box::new(_native));
        let mut stack_size = cmp::max(stack, STACK_GRANULARITY);
        if stack_size % STACK_GRANULARITY != 0 {
            // Pretty sure that alignment is 0x1000 and that is the minimum that you can have soooo..
            stack_size = ((STACK_GRANULARITY - (stack_size % STACK_GRANULARITY)) % STACK_GRANULARITY) + stack_size;
        }
        
        let mut stack_mem: *mut libc::c_void = 0 as *mut libc::c_void;
        libc::posix_memalign(&mut stack_mem, STACK_GRANULARITY, stack_size);
        assert!(stack_mem != 0 as *mut libc::c_void);
        
        let ret = nnsdk::os::CreateThread1(native, thread_start, p as *mut _, stack_mem, stack_size as u64, 31i32);
        return if ret != 0 {
            drop(Box::from_raw(p));
            drop(Box::from_raw(native));
            libc::free(stack_mem);
            Err(io::Error::last_os_error())
        } else {
            nnsdk::os::StartThread(native);
            Ok(Thread { native: native })
        };

        extern "C" fn thread_start(main: *mut libc::c_void) {
            unsafe {
                Box::from_raw(main as *mut Box<dyn FnOnce()>)();
            }
        }

    }

    pub fn yield_now() {
        unsafe {
            nnsdk::os::YieldThread();
        }
    }

    pub fn set_name(name: &CStr) {
        use crate::ffi::CString;
        let cname = CString::new(&b"%s"[..]).unwrap();
        unsafe {
            nnsdk::os::SetThreadName(nnsdk::os::GetCurrentThread(), cname.as_ptr() as *const u8);   
        }
    }

    pub fn sleep(dur: Duration) {
        let time_span = TimeSpan::nano(dur.as_nanos() as u64);
        unsafe {
            nnsdk::os::SleepThread(time_span);
        }
    }

    pub fn join(mut self) {
        unsafe {
            nnsdk::os::WaitThread(self.native);
            nnsdk::os::DestroyThread(self.native);
            drop(Box::from_raw(self.native));
            mem::forget(self);
        }
    }
}

impl Drop for Thread {
    fn drop(&mut self) {
        unsafe {
            // nnsdk::os::DestroyThread(&mut self.native);
            // change this if needed, but i feel like threads should exist beyond when they are dropped.
        }
    }
}

#[cfg_attr(test, allow(dead_code))]
pub mod guard {
    use crate::ops::Range;
    pub type Guard = Range<usize>;
    pub unsafe fn current() -> Option<Guard> {
        None
    }
    pub unsafe fn init() -> Option<Guard> {
        None
    }
}
