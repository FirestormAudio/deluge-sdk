/* Minimal C-runtime support for the embedded wren VM (Milestone 0).
 *
 * The VM allocates exclusively through our custom WrenReallocateFn (routed onto
 * the Deluge SDRAM heap in Rust), so it never touches malloc/_sbrk directly.
 * newlib still mallocs internally for a few incidental paths (e.g. float
 * formatting inside snprintf, which wren uses to stringify numbers), so we give
 * _sbrk a small fixed static arena and the whole libc path works without an OS.
 */

#include <stddef.h>

#ifndef WREN_HEAP_SIZE
#define WREN_HEAP_SIZE (256 * 1024)
#endif

static unsigned char wren_heap[WREN_HEAP_SIZE] __attribute__((aligned(16)));
static unsigned char *wren_brk = wren_heap;

void *_sbrk(ptrdiff_t incr) {
    unsigned char *prev = wren_brk;
    unsigned char *next = wren_brk + incr;
    if (next < wren_heap || next > wren_heap + WREN_HEAP_SIZE) {
        return (void *)-1; /* out of arena */
    }
    wren_brk = next;
    return (void *)prev;
}

/* ARM EH personality routines referenced by newlib's .ARM.exidx tables. This
 * image is built without panic/exception unwinding, so these are never called;
 * the stubs exist only to keep the link free of undefined symbols (we don't
 * link libgcc, which would otherwise supply them and drag in its unwinder). */
int __aeabi_unwind_cpp_pr0(void) { return 0; }
int __aeabi_unwind_cpp_pr1(void) { return 0; }
int __aeabi_unwind_cpp_pr2(void) { return 0; }
