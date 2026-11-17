/* A static musl test program: writes to stdout and reads a file from the drive. */
#include <stdio.h>

int main(void) {
    printf("hello from a musl program\n");

    FILE *f = fopen("/Documents/hello.txt", "r");
    if (!f) {
        printf("could not open /Documents/hello.txt\n");
        return 1;
    }
    char line[128];
    if (fgets(line, sizeof line, f)) {
        printf("the file says: %s", line);
    }
    fclose(f);
    return 0;
}
