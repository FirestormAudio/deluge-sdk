//! FFI surface to the upstream `wren-lang/wren` C VM (compiler + VM), compiled
//! stock in `build.rs`.
//!
//! M0 scope: the raw embedding ABI ([`WrenConfiguration`] + lifecycle + slot
//! calls), a custom [`WrenReallocateFn`] that routes the VM's heap onto the
//! Deluge SDRAM, and a thin [`boot`]/[`interpret`] convenience that stands up a
//! configured VM whose `System.print` / error output is forwarded to two host
//! hooks ([`wren_host_write`], [`wren_host_error`]) the firmware provides.
//!
//! Later milestones add the foreign-object registry (lifted from `wren-rs`'s
//! `foreign.rs`) to bind native Rust classes (`Osc`, `Output`, …).

#![no_std]
#![feature(allocator_api)]

use core::alloc::{Allocator, Layout};
use core::cmp::min;
use core::ffi::{c_char, c_int, c_void};
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicUsize, Ordering};

use deluge_alloc as allocator;

pub mod foreign;
pub use foreign::{ClassEntry, Finalizer, ForeignMethod, MethodEntry, Vm, WrenForeign, WrenType};

// ── Opaque VM handle ────────────────────────────────────────────────────────

/// Opaque `WrenVM`. Only ever held behind a pointer.
#[repr(C)]
pub struct WrenVM {
    _private: [u8; 0],
}

/// Opaque `WrenHandle` — a GC-rooted reference to a wren value (e.g. a callback
/// `Fn`), kept alive until released. Used to call back into wren from Rust.
#[repr(C)]
pub struct WrenHandle {
    _private: [u8; 0],
}

// ── Callback function-pointer types (match `wren.h`) ─────────────────────────

/// `void* (*)(void* memory, size_t newSize, void* userData)`
pub type WrenReallocateFn =
    Option<unsafe extern "C" fn(*mut c_void, usize, *mut c_void) -> *mut c_void>;
/// `void (*)(WrenVM*, const char* text)`
pub type WrenWriteFn = Option<unsafe extern "C" fn(*mut WrenVM, *const c_char)>;
/// `void (*)(WrenVM*, WrenErrorType, const char* module, int line, const char* message)`
pub type WrenErrorFn =
    Option<unsafe extern "C" fn(*mut WrenVM, c_int, *const c_char, c_int, *const c_char)>;

/// VM configuration. Field order/types **must** match `WrenConfiguration` in
/// `wren.h` exactly. The four module/foreign callbacks are unused in M0 and held
/// as raw pointers (ABI-identical to a function pointer); they are typed
/// precisely once the foreign registry lands.
/// `WrenBindForeignMethodFn`: `(vm, module, className, isStatic, signature) ->
/// WrenForeignMethodFn`.
pub type WrenBindForeignMethodFn = Option<
    unsafe extern "C" fn(
        *mut WrenVM,
        *const c_char,
        *const c_char,
        bool,
        *const c_char,
    ) -> Option<ForeignMethod>,
>;
/// `WrenBindForeignClassFn`: `(vm, module, className) -> WrenForeignClassMethods`.
pub type WrenBindForeignClassFn = Option<
    unsafe extern "C" fn(
        *mut WrenVM,
        *const c_char,
        *const c_char,
    ) -> foreign::WrenForeignClassMethods,
>;

#[repr(C)]
pub struct WrenConfiguration {
    pub reallocate_fn: WrenReallocateFn,
    pub resolve_module_fn: *const c_void,
    pub load_module_fn: *const c_void,
    pub bind_foreign_method_fn: WrenBindForeignMethodFn,
    pub bind_foreign_class_fn: WrenBindForeignClassFn,
    pub write_fn: WrenWriteFn,
    pub error_fn: WrenErrorFn,
    pub initial_heap_size: usize,
    pub min_heap_size: usize,
    pub heap_growth_percent: c_int,
    pub user_data: *mut c_void,
}

/// `WrenInterpretResult`
pub const WREN_RESULT_SUCCESS: c_int = 0;
pub const WREN_RESULT_COMPILE_ERROR: c_int = 1;
pub const WREN_RESULT_RUNTIME_ERROR: c_int = 2;

// ── Upstream VM entry points ─────────────────────────────────────────────────

unsafe extern "C" {
    pub fn wrenInitConfiguration(config: *mut WrenConfiguration);
    pub fn wrenNewVM(config: *mut WrenConfiguration) -> *mut WrenVM;
    pub fn wrenFreeVM(vm: *mut WrenVM);
    pub fn wrenCollectGarbage(vm: *mut WrenVM);
    pub fn wrenInterpret(
        vm: *mut WrenVM,
        module: *const c_char,
        source: *const c_char,
    ) -> c_int;

    // Slot API — declared now, used from the foreign registry in later milestones.
    pub fn wrenGetSlotCount(vm: *mut WrenVM) -> c_int;
    pub fn wrenEnsureSlots(vm: *mut WrenVM, num_slots: c_int);
    pub fn wrenGetSlotType(vm: *mut WrenVM, slot: c_int) -> c_int;
    pub fn wrenGetSlotBool(vm: *mut WrenVM, slot: c_int) -> bool;
    pub fn wrenGetSlotDouble(vm: *mut WrenVM, slot: c_int) -> f64;
    pub fn wrenGetSlotString(vm: *mut WrenVM, slot: c_int) -> *const c_char;
    pub fn wrenGetSlotBytes(vm: *mut WrenVM, slot: c_int, length: *mut c_int) -> *const c_char;
    pub fn wrenGetSlotForeign(vm: *mut WrenVM, slot: c_int) -> *mut c_void;
    pub fn wrenSetSlotBool(vm: *mut WrenVM, slot: c_int, value: bool);
    pub fn wrenSetSlotDouble(vm: *mut WrenVM, slot: c_int, value: f64);
    pub fn wrenSetSlotString(vm: *mut WrenVM, slot: c_int, text: *const c_char);
    pub fn wrenSetSlotBytes(vm: *mut WrenVM, slot: c_int, bytes: *const c_char, length: usize);
    pub fn wrenSetSlotNull(vm: *mut WrenVM, slot: c_int);
    pub fn wrenSetSlotNewForeign(
        vm: *mut WrenVM,
        slot: c_int,
        class_slot: c_int,
        size: usize,
    ) -> *mut c_void;
    pub fn wrenGetVariable(
        vm: *mut WrenVM,
        module: *const c_char,
        name: *const c_char,
        slot: c_int,
    );
    pub fn wrenAbortFiber(vm: *mut WrenVM, slot: c_int);

    // Calling wren from Rust (used for Metro/Clock callbacks).
    pub fn wrenMakeCallHandle(vm: *mut WrenVM, signature: *const c_char) -> *mut WrenHandle;
    pub fn wrenCall(vm: *mut WrenVM, method: *mut WrenHandle) -> c_int;
    pub fn wrenReleaseHandle(vm: *mut WrenVM, handle: *mut WrenHandle);
    pub fn wrenGetSlotHandle(vm: *mut WrenVM, slot: c_int) -> *mut WrenHandle;
    pub fn wrenSetSlotHandle(vm: *mut WrenVM, slot: c_int, handle: *mut WrenHandle);
}

// ── Host hooks (provided by the firmware) ────────────────────────────────────

unsafe extern "C" {
    /// Receives the NUL-terminated text from `System.print` and friends.
    fn wren_host_write(text: *const c_char);
    /// Receives a VM error: `line` (-1 when not applicable) + NUL-terminated message.
    fn wren_host_error(line: c_int, message: *const c_char);
    /// Diagnostic numeric trace (M0 bring-up only): `tag` + value.
    fn wren_host_debug(tag: c_int, value: usize);
}

unsafe extern "C" fn write_trampoline(_vm: *mut WrenVM, text: *const c_char) {
    if !text.is_null() {
        unsafe { wren_host_write(text) };
    }
}

unsafe extern "C" fn error_trampoline(
    _vm: *mut WrenVM,
    _err_type: c_int,
    _module: *const c_char,
    line: c_int,
    message: *const c_char,
) {
    if !message.is_null() {
        unsafe { wren_host_error(line, message) };
    }
}

// ── SDRAM-backed reallocate ──────────────────────────────────────────────────
//
// A 16-byte header stores the user size and keeps the returned pointer
// 16-aligned. The VM's whole heap lives in the 64 MB SDRAM, not the small
// static `_sbrk` arena.

/// Header size / allocation alignment for VM allocations.
const HDR: usize = 16;

/// Live VM-owned bytes currently allocated from SDRAM.
static LIVE: AtomicUsize = AtomicUsize::new(0);
/// High-water mark of [`LIVE`].
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// Peak SDRAM bytes the VM allocator has ever had outstanding at once.
pub fn peak_bytes() -> usize {
    PEAK.load(Ordering::Relaxed)
}

#[inline]
fn block_layout(user: usize) -> Layout {
    // SAFETY: HDR is a valid non-zero power-of-two alignment; user + HDR never
    // overflows for any size the VM requests.
    unsafe { Layout::from_size_align_unchecked(user + HDR, HDR) }
}

/// Reject obviously-bogus allocation sizes (the SDRAM heap is 64 MB) so a
/// runaway length surfaces as a logged, clean OOM instead of a hard fault.
const MAX_ALLOC: usize = 48 * 1024 * 1024;

unsafe fn alloc_block(user: usize) -> *mut c_void {
    if user > MAX_ALLOC {
        // M0 diagnostic: a request this large means a corrupt/runaway size.
        unsafe { wren_host_debug(1, user) };
        return ptr::null_mut();
    }
    match allocator::SDRAM.allocate(block_layout(user)) {
        Ok(p) => {
            let base = p.as_ptr() as *mut u8;
            // Store the user size in the header word.
            unsafe { (base as *mut usize).write(user) };
            let live = LIVE.fetch_add(user, Ordering::Relaxed) + user;
            PEAK.fetch_max(live, Ordering::Relaxed);
            unsafe { base.add(HDR) as *mut c_void }
        }
        Err(_) => ptr::null_mut(),
    }
}

unsafe fn free_block(user_ptr: *mut c_void) {
    let base = unsafe { (user_ptr as *mut u8).sub(HDR) };
    let user = unsafe { (base as *const usize).read() };
    LIVE.fetch_sub(user, Ordering::Relaxed);
    // SAFETY: `base` came from `alloc_block` with this exact layout.
    unsafe {
        allocator::SDRAM.deallocate(NonNull::new_unchecked(base), block_layout(user));
    }
}

unsafe extern "C" fn wren_reallocate(
    memory: *mut c_void,
    new_size: usize,
    _user_data: *mut c_void,
) -> *mut c_void {
    if new_size == 0 {
        if !memory.is_null() {
            unsafe { free_block(memory) };
        }
        return ptr::null_mut();
    }
    if memory.is_null() {
        return unsafe { alloc_block(new_size) };
    }
    // Realloc: allocate new, copy min(old, new), free old.
    let base = unsafe { (memory as *mut u8).sub(HDR) };
    let old_size = unsafe { (base as *const usize).read() };
    let np = unsafe { alloc_block(new_size) };
    if np.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        ptr::copy_nonoverlapping(memory as *const u8, np as *mut u8, min(old_size, new_size));
        free_block(memory);
    }
    np
}

// ── Convenience bring-up ─────────────────────────────────────────────────────

/// Build a [`WrenConfiguration`] wired to the SDRAM allocator and the host
/// write/error hooks. Module-import and foreign callbacks are left NULL in M0.
fn make_config() -> WrenConfiguration {
    let mut cfg = WrenConfiguration {
        reallocate_fn: None,
        resolve_module_fn: ptr::null(),
        load_module_fn: ptr::null(),
        bind_foreign_method_fn: None,
        bind_foreign_class_fn: None,
        write_fn: None,
        error_fn: None,
        initial_heap_size: 0,
        min_heap_size: 0,
        heap_growth_percent: 0,
        user_data: ptr::null_mut(),
    };
    // SAFETY: cfg is a valid, correctly-laid-out WrenConfiguration.
    unsafe { wrenInitConfiguration(&mut cfg) };
    cfg.reallocate_fn = Some(wren_reallocate);
    cfg.write_fn = Some(write_trampoline);
    cfg.error_fn = Some(error_trampoline);
    cfg.bind_foreign_method_fn = Some(foreign::bind_method);
    cfg.bind_foreign_class_fn = Some(foreign::bind_class);
    // Modest GC thresholds for the embedded heap (defaults are 10 MB / 1 MB).
    cfg.initial_heap_size = 1 << 20; // 1 MB
    cfg.min_heap_size = 1 << 18; //    256 KB
    cfg
}

/// Create a configured VM with no foreign bindings. Returns NULL on failure.
///
/// # Safety
/// Must be called once from the VM-owning context, after the SDRAM heap is
/// initialised. The returned pointer is owned by the caller (free with
/// [`wrenFreeVM`]).
pub unsafe fn boot() -> *mut WrenVM {
    unsafe { boot_with_foreign(&[], &[]) }
}

/// Create a configured VM with native foreign bindings. The `methods`/`classes`
/// tables must be `'static` (the VM's bind callbacks scan them for the lifetime
/// of the VM).
///
/// # Safety
/// As [`boot`]; call once from the VM-owning context after SDRAM init.
pub unsafe fn boot_with_foreign(
    methods: &'static [MethodEntry],
    classes: &'static [ClassEntry],
) -> *mut WrenVM {
    foreign::set_registry(methods, classes);
    let mut cfg = make_config();
    unsafe { wrenNewVM(&mut cfg) }
}

/// Interpret `source` (a NUL-terminated Wren program) in `module`
/// (NUL-terminated) on `vm`. Returns a `WREN_RESULT_*` code.
///
/// # Safety
/// `vm` must be a live handle from [`boot`]; `module`/`source` must be valid
/// NUL-terminated C strings.
pub unsafe fn interpret(vm: *mut WrenVM, module: *const c_char, source: *const c_char) -> c_int {
    unsafe { wrenInterpret(vm, module, source) }
}
