// SPDX-License-Identifier: MIT OR Apache-2.0
//
// ASCII Mandelbrot renderer for the hart guest. All arithmetic is IEEE 754
// double precision, so the output is byte-for-byte identical on any correct
// RV64GC implementation (and on any host built with -ffp-contract=off).

#include <stdio.h>

#define COLS 72
#define ROWS 28
#define MAX_ITER 1000

static const double re_min = -2.25, re_max = 0.75;
static const double im_min = -1.25, im_max = 1.25;

// Escape-time shading: one character per power of two of the iteration count.
static const char shade[] = " .:-=+*%#";

static char cell(int iter)
{
    if (iter >= MAX_ITER)
        return '@';
    int level = 0;
    while (iter > 1 && level < (int)sizeof(shade) - 2) {
        iter >>= 1;
        level++;
    }
    return shade[level];
}

int main(void)
{
    const double d_re = (re_max - re_min) / COLS;
    const double d_im = (im_max - im_min) / ROWS;
    long inside = 0;
    long total_iter = 0;
    char line[COLS + 1];

    for (int row = 0; row < ROWS; row++) {
        const double ci = im_max - (row + 0.5) * d_im;
        int len = 0;
        for (int col = 0; col < COLS; col++) {
            const double cr = re_min + (col + 0.5) * d_re;
            double zr = 0.0, zi = 0.0, zr2 = 0.0, zi2 = 0.0;
            int iter = 0;
            while (iter < MAX_ITER && zr2 + zi2 <= 4.0) {
                zi = 2.0 * zr * zi + ci;
                zr = zr2 - zi2 + cr;
                zr2 = zr * zr;
                zi2 = zi * zi;
                iter++;
            }
            total_iter += iter;
            if (iter >= MAX_ITER)
                inside++;
            line[col] = cell(iter);
            if (line[col] != ' ')
                len = col + 1;
        }
        // Trailing spaces are trimmed so terminals and screen scrapers agree.
        line[len] = '\0';
        puts(line);
    }

    printf("%dx%d cells, %d iterations max, %ld total; area estimate %.6f\n",
           COLS, ROWS, MAX_ITER, total_iter, inside * d_re * d_im);
    return 0;
}
