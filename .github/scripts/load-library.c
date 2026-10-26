#include <dlfcn.h>
#include <stdio.h>

int main(int argc, char **argv)
{
    if (argc != 3) {
        fprintf(stderr, "Usage: %s /path/to/library.so required-symbol\n", argv[0]);
        return 2;
    }

    void *library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (library == NULL) {
        fprintf(stderr, "Unable to load %s: %s\n", argv[1], dlerror());
        return 1;
    }
    dlerror();
    void *symbol = dlsym(library, argv[2]);
    const char *error = dlerror();
    if (error != NULL || symbol == NULL) {
        fprintf(stderr, "Missing %s in %s: %s\n", argv[2], argv[1],
                error != NULL ? error : "null symbol");
        dlclose(library);
        return 1;
    }
    if (dlclose(library) != 0) {
        fprintf(stderr, "Unable to close %s: %s\n", argv[1], dlerror());
        return 1;
    }
    printf("Successfully loaded %s\n", argv[1]);
    return 0;
}
