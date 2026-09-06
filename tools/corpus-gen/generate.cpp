/*
 * Corpus generator for xisf-rs.
 *
 * Copyright (C) 2026 The xisf-rs Developers
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU General Public License as published by the Free
 * Software Foundation, either version 3 of the License, or (at your option)
 * any later version.  See <http://www.gnu.org/licenses/>.
 *
 * ---------------------------------------------------------------------------
 *
 * NOTE ON LICENSING, WHICH IS THE POINT OF THIS FILE BEING SEPARATE.
 *
 * This is a development tool, not part of the library.  It links libXISF,
 * which is GPL-3.0, so this file is GPL-3.0 too -- a program that links a GPL
 * library is bound by that licence.  It lives under tools/ and nothing in
 * crates/ depends on it, builds it, or links it.  The MIT library and this
 * GPL tool are separate works that never meet in a binary.
 *
 * What it produces -- .xisf files -- is data, not code.  Running a program to
 * generate output creates no licensing obligation on the output, which is why
 * the corpus it writes can sit in an MIT repository and be read by MIT tests.
 * That is the whole reason for generating a corpus this way rather than
 * hand-writing fixtures: the files come from an implementation that is not
 * ours, so they test what the format is rather than what we assumed.
 */

#include <cstdint>
#include <cstring>
#include <filesystem>
#include <iostream>
#include <string>
#include <vector>

#include <libxisf.h>

namespace {

struct FormatCase {
    const char *name;
    LibXISF::Image::SampleFormat format;
    size_t bytes_per_sample;
};

struct CodecCase {
    const char *name;
    LibXISF::DataBlock::CompressionCodec codec;
    bool shuffle;
};

/* Deterministic sample data: the corpus must be reproducible, so nothing here
 * is random.  The pattern varies along all three axes so that a reader that
 * transposes width and height, or loses a channel, produces different bytes. */
void fill(void *data, size_t bytes, size_t bytes_per_sample) {
    auto *out = static_cast<uint8_t *>(data);
    for (size_t i = 0; i < bytes; i++)
        out[i] = static_cast<uint8_t>((i * 37 + (i / bytes_per_sample) * 11 + 1) & 0xff);
}

bool write_one(const std::filesystem::path &dir, const FormatCase &fmt, const CodecCase &codec,
               LibXISF::Image::ColorSpace space, const char *space_name, uint64_t width,
               uint64_t height, uint64_t channels) {
    std::string name = std::string(fmt.name) + "_" + space_name + "_" + codec.name + ".xisf";
    std::filesystem::path path = dir / name;

    try {
        LibXISF::Image image(width, height, channels, fmt.format, space);
        fill(image.imageData(), image.imageDataSize(), fmt.bytes_per_sample);

        image.setCompression(codec.codec);
        image.setByteshuffling(codec.shuffle);

        LibXISF::XISFWriter writer;
        writer.writeImage(image);
        writer.save(path);
    } catch (const std::exception &e) {
        std::cerr << "  SKIP " << name << ": " << e.what() << "\n";
        return false;
    }

    std::cout << "  " << name << " (" << std::filesystem::file_size(path) << " bytes)\n";
    return true;
}

} // namespace

int main(int argc, char **argv) {
    if (argc != 2) {
        std::cerr << "usage: generate <output-directory>\n";
        return 2;
    }
    std::filesystem::path dir = argv[1];
    std::filesystem::create_directories(dir);

    const FormatCase formats[] = {
        {"UInt8", LibXISF::Image::UInt8, 1},     {"UInt16", LibXISF::Image::UInt16, 2},
        {"UInt32", LibXISF::Image::UInt32, 4},   {"Float32", LibXISF::Image::Float32, 4},
        {"Float64", LibXISF::Image::Float64, 8},
    };
    const CodecCase codecs[] = {
        {"none", LibXISF::DataBlock::None, false},
        {"zlib", LibXISF::DataBlock::Zlib, false},
        {"zlib+sh", LibXISF::DataBlock::Zlib, true},
        {"lz4", LibXISF::DataBlock::LZ4, false},
        {"lz4+sh", LibXISF::DataBlock::LZ4, true},
        {"lz4hc", LibXISF::DataBlock::LZ4HC, false},
        {"zstd", LibXISF::DataBlock::ZSTD, false},
    };

    int written = 0;
    std::cout << "generating into " << dir << "\n";
    for (const auto &fmt : formats) {
        for (const auto &codec : codecs) {
            /* Gray keeps most cases small; one RGB per format proves the
             * channel count reaches the geometry attribute. */
            written += write_one(dir, fmt, codec, LibXISF::Image::Gray, "Gray", 17, 11, 1);
        }
        written += write_one(dir, formats[0].name == fmt.name ? fmt : fmt,
                             codecs[0], LibXISF::Image::RGB, "RGB", 5, 7, 3);
    }

    std::cout << written << " files written\n";
    return written > 0 ? 0 : 1;
}
