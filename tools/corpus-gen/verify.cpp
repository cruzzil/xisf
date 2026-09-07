/*
 * Round-trip verifier for xisf-rs.
 *
 * Copyright (C) 2026 The xisf-rs Developers
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU General Public License as published by the Free
 * Software Foundation, either version 3 of the License, or (at your option)
 * any later version.  See <http://www.gnu.org/licenses/>.
 *
 * As with generate.cpp: this links libXISF and is therefore GPL-3.0.  It is a
 * development tool, nothing under crates/ builds or links it, and the MIT
 * library never meets it in a binary.
 *
 * ---------------------------------------------------------------------------
 *
 * The corpus tests prove we can read what libXISF writes.  This proves the
 * other direction, which no test written in Rust can: that libXISF can read
 * what *we* write.  A format implementation that only agrees with itself has
 * shown nothing, and the two directions fail differently -- a reader that is
 * wrong about a field usually still round-trips through its own writer.
 *
 * Usage: verify <file.xisf> <expected-byte-count> [--ancillary]
 *
 * Reads the file with libXISF and prints the image geometry and data size it
 * sees.  Exits non-zero if libXISF refuses the file or disagrees about the
 * size, so a test can simply check the exit status.
 *
 * With --ancillary it additionally requires that libXISF found the ICC
 * profile, thumbnail and FITS keywords we wrote.  Parsing a file is a weaker
 * claim than reading what is in it: an element another implementation cannot
 * see is, for that implementation, an element that is not there.
 */

#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <string>

#include <libxisf.h>

int main(int argc, char **argv) {
    if (argc < 2) {
        std::cerr << "usage: verify <file.xisf> [expected-byte-count]\n";
        return 2;
    }
    const std::string path = argv[1];
    const long long expected = argc > 2 ? std::atoll(argv[2]) : -1;
    bool ancillary = false;
    for (int i = 2; i < argc; ++i)
        if (std::string(argv[i]) == "--ancillary") ancillary = true;

    try {
        LibXISF::XISFReader reader;
        reader.open(path);

        const uint64_t count = reader.imagesCount();
        if (count == 0) {
            std::cerr << "libXISF found no images in " << path << "\n";
            return 1;
        }

        const LibXISF::Image &image = reader.getImage(0);
        const size_t size = image.imageDataSize();

        std::cout << path << ": libXISF read " << count << " image(s), "
                  << image.width() << "x" << image.height() << "x" << image.channelCount()
                  << ", " << size << " bytes\n";

        if (expected >= 0 && static_cast<long long>(size) != expected) {
            std::cerr << "  size mismatch: libXISF says " << size << ", expected " << expected
                      << "\n";
            return 1;
        }

        if (ancillary) {
            if (image.iccProfile().size() == 0) {
                std::cerr << "  libXISF found no ICC profile\n";
                return 1;
            }
            if (image.fitsKeywords().empty()) {
                std::cerr << "  libXISF found no FITS keywords\n";
                return 1;
            }
            const LibXISF::Image &thumb = reader.getThumbnail();
            if (thumb.width() == 0 || thumb.height() == 0) {
                std::cerr << "  libXISF found no thumbnail\n";
                return 1;
            }
            std::cout << "  ancillary: " << image.iccProfile().size() << "-byte ICC profile, "
                      << image.fitsKeywords().size() << " FITS keyword(s), " << thumb.width()
                      << "x" << thumb.height() << " thumbnail\n";
        }
    } catch (const std::exception &e) {
        std::cerr << path << ": libXISF refused the file: " << e.what() << "\n";
        return 1;
    }

    return 0;
}
