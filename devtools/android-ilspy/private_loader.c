#include <dlfcn.h>
#include <string.h>

void *__real_dlopen(const char *name, int flags);

/* Link-local routing inside the engine; not a process-wide loader hook. */
__attribute__((visibility("hidden")))
void *__wrap_dlopen(const char *name, int flags) {
    if (name && strncmp(name, "libssl.so", 9) == 0 &&
        (name[9] == '\0' || name[9] == '.')) {
        return __real_dlopen("librxdopenssl.so", flags);
    }
    return __real_dlopen(name, flags);
}
