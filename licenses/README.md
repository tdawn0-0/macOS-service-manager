# Third-party notices

Generated from the pinned app/CLI production `Cargo.lock` graphs for macOS arm64.
Each component retains its original license and copyright. Nested notice files are copied as provided.
This inventory does not relicense components or claim that every copied file is linked into the executable.
Crates that publish no license file are attributed by their declared crates.io license expression.
Downloaded development runtimes/databases are separate; see docs/third-party-software.md in the source repository.

Regenerate: `python3 scripts/collect-third-party-notices.py`; verify: add `--check`.
Run `cargo fetch --locked` in `app/` and each crate under `app/crates/` first; generation makes no network request.
The SHA-256 inventory is in [manifest.json](manifest.json). No local cache paths are included.

| Crate | Version | License | Copied notices |
| --- | --- | --- | --- |
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 | 3 |
| aead | 0.6.1 | MIT OR Apache-2.0 | 2 |
| aes | 0.9.4 | MIT OR Apache-2.0 | 2 |
| aes-gcm | 0.11.1 | Apache-2.0 OR MIT | 2 |
| aes-kw | 0.3.1 | MIT OR Apache-2.0 | 2 |
| aho-corasick | 1.1.5 | Unlicense OR MIT | 3 |
| alloc-no-stdlib | 3.0.0 | BSD-3-Clause | 1 |
| alloc-stdlib | 0.3.0 | BSD-3-Clause | 0 |
| allocator-api2 | 0.2.21 | MIT OR Apache-2.0 | 2 |
| ambient-authority | 0.0.2 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 4 |
| annotate-snippets | 0.12.18 | MIT OR Apache-2.0 | 2 |
| anstream | 1.0.0 | MIT OR Apache-2.0 | 2 |
| anstyle | 1.0.14 | MIT OR Apache-2.0 | 2 |
| anstyle-parse | 1.0.0 | MIT OR Apache-2.0 | 2 |
| anstyle-query | 1.1.5 | MIT OR Apache-2.0 | 2 |
| arc-swap | 1.9.5 | MIT OR Apache-2.0 | 2 |
| arraydeque | 0.5.1 | MIT/Apache-2.0 | 1 |
| arrayref | 0.3.9 | BSD-2-Clause | 1 |
| arrayvec | 0.7.8 | MIT OR Apache-2.0 | 2 |
| ash | 0.38.0+1.3.281 | MIT OR Apache-2.0 | 2 |
| asn1-rs | 0.7.2 | MIT OR Apache-2.0 | 2 |
| asn1-rs-derive | 0.6.0 | MIT OR Apache-2.0 | 2 |
| asn1-rs-impl | 0.2.0 | MIT/Apache-2.0 | 0 |
| async-lock | 3.5.0 | Apache-2.0 OR MIT | 2 |
| atomic-waker | 1.1.2 | Apache-2.0 OR MIT | 3 |
| autocfg | 1.5.1 | Apache-2.0 OR MIT | 2 |
| aws-lc-rs | 1.19.0 | ISC AND (Apache-2.0 OR ISC) | 1 |
| aws-lc-sys | 0.47.0 | ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0) | 3 |
| base16ct | 1.0.0 | Apache-2.0 OR MIT | 2 |
| base62 | 2.2.6 | MIT | 1 |
| base64 | 0.22.1 | MIT OR Apache-2.0 | 2 |
| base64 | 0.23.1 | MIT OR Apache-2.0 | 2 |
| base64-simd | 0.8.0 | MIT | 0 |
| base64ct | 1.8.3 | Apache-2.0 OR MIT | 2 |
| bincode | 1.3.3 | MIT | 1 |
| bindgen | 0.72.1 | BSD-3-Clause | 1 |
| bit-set | 0.8.0 | Apache-2.0 OR MIT | 2 |
| bit-set | 0.9.1 | Apache-2.0 OR MIT | 2 |
| bit-vec | 0.8.0 | Apache-2.0 OR MIT | 2 |
| bit-vec | 0.9.1 | Apache-2.0 OR MIT | 2 |
| bitflags | 1.3.2 | MIT/Apache-2.0 | 2 |
| bitflags | 2.13.2 | MIT OR Apache-2.0 | 2 |
| block-buffer | 0.10.4 | MIT OR Apache-2.0 | 2 |
| block-buffer | 0.12.1 | MIT OR Apache-2.0 | 2 |
| block-padding | 0.4.2 | MIT OR Apache-2.0 | 2 |
| block2 | 0.6.2 | MIT | 0 |
| brotli | 9.1.3 | BSD-3-Clause AND MIT | 2 |
| brotli-decompressor | 6.3.2 | BSD-3-Clause/MIT | 1 |
| bstr | 1.14.0 | MIT OR Apache-2.0 | 3 |
| bumpalo | 3.20.4 | MIT OR Apache-2.0 | 2 |
| bytemuck | 1.25.2 | Zlib OR Apache-2.0 OR MIT | 3 |
| bytemuck_derive | 1.12.1 | Zlib OR Apache-2.0 OR MIT | 3 |
| byteorder-lite | 0.1.0 | Unlicense OR MIT | 2 |
| bytes | 1.12.2 | MIT | 1 |
| cap-primitives | 4.0.3 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 6 |
| cap-std | 4.0.3 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 4 |
| cbc | 0.2.1 | MIT OR Apache-2.0 | 2 |
| cc | 1.11.2 | MIT OR Apache-2.0 | 2 |
| cexpr | 0.6.0 | Apache-2.0/MIT | 2 |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 | 2 |
| cfg_aliases | 0.2.2 | MIT | 2 |
| chacha20 | 0.10.2 | MIT OR Apache-2.0 | 2 |
| chacha20poly1305 | 0.11.0 | Apache-2.0 OR MIT | 2 |
| chrono | 0.4.45 | MIT OR Apache-2.0 | 1 |
| cipher | 0.5.2 | MIT OR Apache-2.0 | 2 |
| clang-sys | 1.9.1 | Apache-2.0 | 1 |
| cmake | 0.1.58 | MIT OR Apache-2.0 | 2 |
| cmov | 0.5.4 | Apache-2.0 OR MIT | 2 |
| codespan-reporting | 0.13.1 | Apache-2.0 | 1 |
| color_quant | 1.1.0 | MIT | 1 |
| colorchoice | 1.0.5 | MIT OR Apache-2.0 | 2 |
| const-oid | 0.10.3 | Apache-2.0 OR MIT | 2 |
| const-oid | 0.9.6 | Apache-2.0 OR MIT | 2 |
| constant_time_eq | 0.6.1 | CC0-1.0 OR MIT-0 OR Apache-2.0 | 3 |
| convert_case | 0.12.0 | MIT | 1 |
| core-foundation | 0.10.1 | MIT OR Apache-2.0 | 2 |
| core-foundation-sys | 0.8.7 | MIT OR Apache-2.0 | 2 |
| core_maths | 0.1.1 | MIT | 1 |
| cpubits | 0.1.1 | MIT OR Apache-2.0 | 2 |
| cpufeatures | 0.2.17 | MIT OR Apache-2.0 | 2 |
| cpufeatures | 0.3.1 | MIT OR Apache-2.0 | 2 |
| crc32c | 0.6.9 | MIT OR Apache-2.0 | 2 |
| crc32fast | 1.5.5 | MIT OR Apache-2.0 | 2 |
| crossbeam-channel | 0.5.17 | MIT OR Apache-2.0 | 3 |
| crossbeam-deque | 0.8.8 | MIT OR Apache-2.0 | 2 |
| crossbeam-epoch | 0.9.21 | MIT OR Apache-2.0 | 2 |
| crossbeam-utils | 0.8.23 | MIT OR Apache-2.0 | 2 |
| crypto-bigint | 0.7.11 | Apache-2.0 OR MIT | 2 |
| crypto-common | 0.1.7 | MIT OR Apache-2.0 | 2 |
| crypto-common | 0.2.2 | MIT OR Apache-2.0 | 2 |
| crypto-primes | 0.7.2 | Apache-2.0 OR MIT | 2 |
| cshake | 0.2.1 | MIT OR Apache-2.0 | 2 |
| ctr | 0.10.1 | MIT OR Apache-2.0 | 2 |
| ctutils | 0.4.8 | Apache-2.0 OR MIT | 2 |
| curve25519-dalek | 4.1.3 | BSD-3-Clause | 1 |
| curve25519-dalek | 5.0.0 | BSD-3-Clause | 1 |
| data-encoding | 2.11.1 | MIT | 1 |
| data-url | 0.3.2 | MIT OR Apache-2.0 | 2 |
| der | 0.7.10 | Apache-2.0 OR MIT | 2 |
| der | 0.8.3 | Apache-2.0 OR MIT | 2 |
| der-parser | 10.0.0 | MIT OR Apache-2.0 | 2 |
| der_derive | 0.8.0 | Apache-2.0 OR MIT | 2 |
| deranged | 0.5.9 | MIT OR Apache-2.0 | 2 |
| digest | 0.10.7 | MIT OR Apache-2.0 | 2 |
| digest | 0.11.3 | MIT OR Apache-2.0 | 2 |
| dispatch2 | 0.3.1 | Zlib OR Apache-2.0 OR MIT | 0 |
| displaydoc | 0.2.7 | MIT OR Apache-2.0 | 2 |
| dlib | 0.5.3 | MIT | 1 |
| document-features | 0.2.12 | MIT OR Apache-2.0 | 2 |
| dunce | 1.0.5 | CC0-1.0 OR MIT-0 OR Apache-2.0 | 1 |
| ecdsa | 0.17.0 | Apache-2.0 OR MIT | 2 |
| ed25519 | 2.2.3 | Apache-2.0 OR MIT | 2 |
| ed25519 | 3.0.0 | Apache-2.0 OR MIT | 2 |
| ed25519-dalek | 2.2.0 | BSD-3-Clause | 1 |
| ed25519-dalek | 3.0.0 | BSD-3-Clause | 1 |
| either | 1.19.0 | MIT OR Apache-2.0 | 2 |
| elliptic-curve | 0.14.3 | Apache-2.0 OR MIT | 2 |
| encoding_rs | 0.8.44 | (Apache-2.0 OR MIT) AND BSD-3-Clause | 4 |
| encoding_rs_io | 0.1.8 | MIT OR Apache-2.0 | 3 |
| env_filter | 2.0.0 | MIT OR Apache-2.0 | 2 |
| env_logger | 0.11.11 | MIT OR Apache-2.0 | 2 |
| equivalent | 1.0.3 | Apache-2.0 OR MIT | 2 |
| errno | 0.3.15 | MIT OR Apache-2.0 | 2 |
| escape-simd | 0.1.1 | MIT AND Apache-2.0 | 3 |
| euclid | 0.22.14 | MIT OR Apache-2.0 | 3 |
| event-listener | 5.4.2 | Apache-2.0 OR MIT | 2 |
| event-listener-strategy | 0.5.4 | Apache-2.0 OR MIT | 2 |
| fancy-regex | 0.16.2 | MIT | 2 |
| fancy-regex | 0.20.2 | MIT | 2 |
| fastrand | 2.5.0 | Apache-2.0 OR MIT | 2 |
| fdeflate | 0.3.7 | MIT OR Apache-2.0 | 2 |
| ff | 0.14.0 | MIT/Apache-2.0 | 2 |
| filetime | 0.2.30 | MIT/Apache-2.0 | 2 |
| find-msvc-tools | 0.1.15 | MIT OR Apache-2.0 | 2 |
| flate2 | 1.1.11 | MIT OR Apache-2.0 | 2 |
| float-cmp | 0.10.0 | MIT | 1 |
| float-cmp | 0.9.0 | MIT | 1 |
| fnv | 1.0.7 | Apache-2.0 / MIT | 2 |
| foldhash | 0.1.5 | Zlib | 1 |
| foldhash | 0.2.0 | Zlib | 1 |
| fontdb | 0.23.0 | MIT | 2 |
| form_urlencoded | 1.2.2 | MIT OR Apache-2.0 | 2 |
| fs-set-times | 0.20.3 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 4 |
| fs_extra | 1.3.0 | MIT | 1 |
| futf | 0.1.5 | MIT / Apache-2.0 | 2 |
| futures-channel | 0.3.34 | MIT OR Apache-2.0 | 2 |
| futures-core | 0.3.34 | MIT OR Apache-2.0 | 2 |
| futures-macro | 0.3.34 | MIT OR Apache-2.0 | 2 |
| futures-sink | 0.3.34 | MIT OR Apache-2.0 | 2 |
| futures-task | 0.3.34 | MIT OR Apache-2.0 | 2 |
| futures-util | 0.3.34 | MIT OR Apache-2.0 | 2 |
| generic-array | 0.14.7 | MIT | 1 |
| getrandom | 0.2.17 | MIT OR Apache-2.0 | 2 |
| getrandom | 0.3.4 | MIT OR Apache-2.0 | 2 |
| getrandom | 0.4.3 | MIT OR Apache-2.0 | 2 |
| ghash | 0.6.0 | Apache-2.0 OR MIT | 2 |
| gif | 0.14.2 | MIT OR Apache-2.0 | 2 |
| glob | 0.3.4 | MIT OR Apache-2.0 | 2 |
| globset | 0.4.21 | Unlicense OR MIT | 3 |
| globwalk | 0.8.1 | MIT | 1 |
| glow | 0.17.0 | MIT OR Apache-2.0 OR Zlib | 3 |
| gpu-allocator | 0.28.0 | MIT OR Apache-2.0 | 2 |
| gpu-descriptor | 0.3.2 | MIT OR Apache-2.0 | 0 |
| gpu-descriptor-types | 0.2.0 | MIT OR Apache-2.0 | 0 |
| granit-parser | 1.3.0 | MIT OR Apache-2.0 | 1 |
| group | 0.14.0 | MIT/Apache-2.0 | 3 |
| h2 | 0.4.20 | MIT | 1 |
| half | 2.7.1 | MIT OR Apache-2.0 | 2 |
| halfbrown | 0.4.0 | Apache-2.0/MIT | 2 |
| hashbrown | 0.15.5 | MIT OR Apache-2.0 | 2 |
| hashbrown | 0.16.1 | MIT OR Apache-2.0 | 2 |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 | 2 |
| hex | 0.4.3 | MIT OR Apache-2.0 | 2 |
| hex-simd | 0.8.0 | MIT | 0 |
| hexf-parse | 0.2.1 | CC0-1.0 | 0 |
| hkdf | 0.13.0 | MIT OR Apache-2.0 | 2 |
| hmac | 0.12.1 | MIT OR Apache-2.0 | 2 |
| hmac | 0.13.0 | MIT OR Apache-2.0 | 2 |
| html5ever | 0.27.0 | MIT OR Apache-2.0 | 2 |
| http | 1.5.0 | MIT OR Apache-2.0 | 2 |
| http-body | 1.1.0 | MIT | 1 |
| http-body-util | 0.1.5 | MIT | 1 |
| httparse | 1.10.1 | MIT OR Apache-2.0 | 2 |
| httpdate | 1.0.3 | MIT OR Apache-2.0 | 2 |
| hybrid-array | 0.4.15 | MIT OR Apache-2.0 | 2 |
| hyper | 1.12.0 | MIT | 1 |
| hyper-rustls | 0.27.10 | Apache-2.0 OR ISC OR MIT | 3 |
| hyper-util | 0.1.21 | MIT | 1 |
| iana-time-zone | 0.1.65 | MIT OR Apache-2.0 | 2 |
| icu_collections | 2.4.0 | Unicode-3.0 | 1 |
| icu_locale_core | 2.4.0 | Unicode-3.0 | 1 |
| icu_normalizer | 2.4.0 | Unicode-3.0 | 1 |
| icu_normalizer_data | 2.4.0 | Unicode-3.0 | 1 |
| icu_properties | 2.4.0 | Unicode-3.0 | 1 |
| icu_properties_data | 2.4.0 | Unicode-3.0 | 1 |
| icu_provider | 2.4.0 | Unicode-3.0 | 1 |
| idna | 1.1.0 | MIT OR Apache-2.0 | 2 |
| idna_adapter | 1.2.2 | Apache-2.0 OR MIT | 2 |
| ignore | 0.4.34 | Unlicense OR MIT | 3 |
| image-webp | 0.2.4 | MIT OR Apache-2.0 | 2 |
| imagesize | 0.14.0 | MIT | 1 |
| imagesize | 0.15.0 | MIT | 1 |
| imara-diff | 0.2.0 | Apache-2.0 | 1 |
| indexmap | 2.14.2 | Apache-2.0 OR MIT | 2 |
| inout | 0.2.6 | MIT OR Apache-2.0 | 2 |
| io-extras | 0.19.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 4 |
| io-lifetimes | 2.0.4 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 4 |
| io-lifetimes | 3.0.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 4 |
| ipnet | 2.12.2 | MIT OR Apache-2.0 | 2 |
| is_terminal_polyfill | 1.70.2 | MIT OR Apache-2.0 | 2 |
| itertools | 0.11.0 | MIT OR Apache-2.0 | 2 |
| itoa | 1.0.19 | MIT OR Apache-2.0 | 2 |
| jiff | 0.2.46 | Unlicense OR MIT | 3 |
| jiff-core | 0.1.8 | Unlicense OR MIT | 3 |
| jobserver | 0.1.35 | MIT OR Apache-2.0 | 2 |
| json-escape-simd | 3.1.3 | MIT | 4 |
| keccak | 0.2.2 | Apache-2.0 OR MIT | 2 |
| khronos-egl | 6.0.0 | MIT/Apache-2.0 | 2 |
| kurbo | 0.13.1 | Apache-2.0 OR MIT | 2 |
| lazy_static | 1.5.1 | MIT OR Apache-2.0 | 2 |
| libc | 0.2.191 | MIT OR Apache-2.0 | 2 |
| libloading | 0.8.9 | ISC | 1 |
| libm | 0.2.16 | MIT | 1 |
| litemap | 0.8.3 | Unicode-3.0 | 1 |
| litrs | 1.0.1 | MIT OR Apache-2.0 | 2 |
| lock_api | 0.4.14 | MIT OR Apache-2.0 | 2 |
| log | 0.4.35 | MIT OR Apache-2.0 | 2 |
| lru-slab | 0.1.3 | MIT OR Apache-2.0 OR Zlib | 3 |
| mac | 0.1.1 | MIT/Apache-2.0 | 0 |
| markdown | 1.0.0 | MIT | 1 |
| markup5ever | 0.12.1 | MIT OR Apache-2.0 | 2 |
| markup5ever_rcdom | 0.3.0 | MIT OR Apache-2.0 | 5 |
| maybe-owned | 0.3.4 | MIT OR Apache-2.0 | 2 |
| md-5 | 0.11.0 | MIT OR Apache-2.0 | 2 |
| memchr | 2.8.3 | Unlicense OR MIT | 3 |
| memmap2 | 0.9.12 | MIT OR Apache-2.0 | 2 |
| minimal-lexical | 0.2.1 | MIT/Apache-2.0 | 3 |
| miniz_oxide | 0.8.9 | MIT OR Zlib OR Apache-2.0 | 4 |
| miniz_oxide | 0.9.1 | MIT OR Zlib OR Apache-2.0 | 4 |
| mio | 1.2.5 | MIT | 1 |
| multiversion_no_op | 1.0.0 | Apache-2.0 OR MIT | 3 |
| naga | 29.0.4 | MIT OR Apache-2.0 | 2 |
| new_debug_unreachable | 1.0.6 | MIT | 1 |
| nix | 0.32.0 | MIT | 1 |
| nohash-hasher | 0.2.0 | Apache-2.0 OR MIT | 2 |
| nom | 7.1.3 | MIT | 1 |
| normpath | 1.5.2 | MIT OR Apache-2.0 | 4 |
| nsh-base | 0.6.1 | Apache-2.0 | 0 |
| nsh-core | 0.6.1 | Apache-2.0 | 0 |
| nsh-engine | 0.6.1 | Apache-2.0 | 0 |
| nsh-i18n | 0.6.1 | Apache-2.0 | 0 |
| nsh-js | 0.6.1 | Apache-2.0 | 1 |
| nsh-macros | 0.6.1 | Apache-2.0 | 0 |
| nsh-package | 0.6.1 | Apache-2.0 | 0 |
| nsh-platform-apple | 0.6.1 | Apache-2.0 | 0 |
| nsh-platform-macos | 0.6.1 | Apache-2.0 | 0 |
| nsh-updater | 0.6.1 | Apache-2.0 | 0 |
| num-bigint | 0.4.8 | MIT OR Apache-2.0 | 2 |
| num-conv | 0.2.2 | MIT OR Apache-2.0 | 2 |
| num-integer | 0.1.47 | MIT OR Apache-2.0 | 2 |
| num-traits | 0.2.19 | MIT OR Apache-2.0 | 2 |
| objc2 | 0.6.5 | MIT | 0 |
| objc2-app-kit | 0.3.2 | Zlib OR Apache-2.0 OR MIT | 0 |
| objc2-core-foundation | 0.3.2 | Zlib OR Apache-2.0 OR MIT | 0 |
| objc2-core-graphics | 0.3.2 | Zlib OR Apache-2.0 OR MIT | 0 |
| objc2-core-text | 0.3.2 | Zlib OR Apache-2.0 OR MIT | 0 |
| objc2-encode | 4.1.0 | MIT | 0 |
| objc2-foundation | 0.3.2 | MIT | 2 |
| objc2-image-io | 0.3.2 | Zlib OR Apache-2.0 OR MIT | 0 |
| objc2-io-kit | 0.3.2 | Zlib OR Apache-2.0 OR MIT | 0 |
| objc2-io-surface | 0.3.2 | Zlib OR Apache-2.0 OR MIT | 0 |
| objc2-metal | 0.3.2 | Zlib OR Apache-2.0 OR MIT | 0 |
| objc2-quartz-core | 0.3.2 | Zlib OR Apache-2.0 OR MIT | 0 |
| oid-registry | 0.8.1 | MIT OR Apache-2.0 | 2 |
| once_cell | 1.21.4 | MIT OR Apache-2.0 | 2 |
| onig | 6.5.3 | MIT | 1 |
| onig_sys | 69.9.3 | MIT | 3 |
| ordered-float | 5.5.1 | MIT | 1 |
| outref | 0.5.2 | MIT | 1 |
| oxc_sourcemap | 9.0.0 | BSD-3-Clause | 1 |
| p256 | 0.14.0 | Apache-2.0 OR MIT | 2 |
| p384 | 0.14.0 | Apache-2.0 OR MIT | 2 |
| p521 | 0.14.0 | Apache-2.0 OR MIT | 2 |
| parking_lot | 0.12.5 | MIT OR Apache-2.0 | 2 |
| parking_lot_core | 0.9.12 | MIT OR Apache-2.0 | 2 |
| pbkdf2 | 0.13.0 | MIT OR Apache-2.0 | 2 |
| pem | 4.0.0 | MIT | 1 |
| pem-rfc7468 | 1.0.0 | Apache-2.0 OR MIT | 2 |
| percent-encoding | 2.3.2 | MIT OR Apache-2.0 | 2 |
| phf | 0.11.3 | MIT | 1 |
| phf | 0.13.1 | MIT | 1 |
| phf | 0.14.0 | MIT | 1 |
| phf_codegen | 0.11.3 | MIT | 1 |
| phf_generator | 0.11.3 | MIT | 1 |
| phf_generator | 0.13.1 | MIT | 1 |
| phf_generator | 0.14.0 | MIT | 1 |
| phf_macros | 0.13.1 | MIT | 1 |
| phf_macros | 0.14.0 | MIT | 1 |
| phf_shared | 0.11.3 | MIT | 1 |
| phf_shared | 0.13.1 | MIT | 1 |
| phf_shared | 0.14.0 | MIT | 1 |
| pico-args | 0.5.0 | MIT | 1 |
| pin-project-lite | 0.2.18 | Apache-2.0 OR MIT | 2 |
| pkcs1 | 0.8.0 | Apache-2.0 OR MIT | 2 |
| pkcs8 | 0.10.2 | Apache-2.0 OR MIT | 2 |
| pkcs8 | 0.11.0 | Apache-2.0 OR MIT | 2 |
| pkg-config | 0.3.34 | MIT OR Apache-2.0 | 2 |
| plist | 1.10.4 | MIT | 1 |
| png | 0.17.16 | MIT OR Apache-2.0 | 2 |
| pollster | 0.4.0 | Apache-2.0/MIT | 2 |
| poly1305 | 0.9.1 | Apache-2.0 OR MIT | 2 |
| polycool | 0.4.0 | MIT OR Apache-2.0 | 2 |
| polyval | 0.7.3 | Apache-2.0 OR MIT | 2 |
| potential_utf | 0.1.6 | Unicode-3.0 | 1 |
| powerfmt | 0.2.1 | MIT OR Apache-2.0 | 2 |
| ppv-lite86 | 0.2.21 | MIT OR Apache-2.0 | 2 |
| precomputed-hash | 0.1.1 | MIT | 1 |
| presser | 0.3.1 | MIT OR Apache-2.0 | 2 |
| prettyplease | 0.2.37 | MIT OR Apache-2.0 | 2 |
| primefield | 0.14.0 | Apache-2.0 OR MIT | 2 |
| primeorder | 0.14.0 | Apache-2.0 OR MIT | 2 |
| proc-macro-crate | 3.5.0 | MIT OR Apache-2.0 | 2 |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 | 2 |
| profiling | 1.0.18 | MIT OR Apache-2.0 | 0 |
| quick-error | 2.0.1 | MIT/Apache-2.0 | 2 |
| quick-xml | 0.42.0 | MIT | 1 |
| quickjs-jit | 0.14.2 | MIT | 1 |
| quickjs-jit-core | 0.14.2 | MIT | 0 |
| quickjs-jit-macro | 0.14.2 | MIT | 0 |
| quickjs-jit-stdlib | 0.14.2 | Apache-2.0 | 2 |
| quickjs-jit-sys | 0.14.2 | MIT | 2 |
| quinn | 0.11.12 | MIT OR Apache-2.0 | 2 |
| quinn-proto | 0.11.19 | MIT OR Apache-2.0 | 2 |
| quinn-udp | 0.5.16 | MIT OR Apache-2.0 | 2 |
| quote | 1.0.47 | MIT OR Apache-2.0 | 2 |
| rand | 0.10.4 | MIT OR Apache-2.0 | 3 |
| rand | 0.8.8 | MIT OR Apache-2.0 | 3 |
| rand | 0.9.5 | MIT OR Apache-2.0 | 3 |
| rand_chacha | 0.3.1 | MIT OR Apache-2.0 | 3 |
| rand_chacha | 0.9.0 | MIT OR Apache-2.0 | 3 |
| rand_core | 0.10.1 | MIT OR Apache-2.0 | 3 |
| rand_core | 0.6.4 | MIT OR Apache-2.0 | 3 |
| rand_core | 0.9.5 | MIT OR Apache-2.0 | 3 |
| rand_pcg | 0.10.3 | MIT OR Apache-2.0 | 3 |
| ratex-font | 0.1.16 | MIT | 0 |
| ratex-layout | 0.1.16 | MIT | 0 |
| ratex-lexer | 0.1.16 | MIT | 0 |
| ratex-parser | 0.1.16 | MIT | 0 |
| ratex-types | 0.1.16 | MIT | 0 |
| raw-window-handle | 0.6.2 | MIT OR Apache-2.0 OR Zlib | 3 |
| raw-window-metal | 1.1.0 | MIT OR Apache-2.0 | 2 |
| rcgen | 0.14.10 | MIT OR Apache-2.0 | 1 |
| regex | 1.13.2 | MIT OR Apache-2.0 | 2 |
| regex-automata | 0.4.19 | MIT OR Apache-2.0 | 2 |
| regex-lite | 0.1.9 | MIT OR Apache-2.0 | 2 |
| regex-syntax | 0.8.12 | MIT OR Apache-2.0 | 3 |
| relative-path | 2.0.3 | MIT OR Apache-2.0 | 2 |
| renderdoc-sys | 1.1.0 | MIT OR Apache-2.0 | 2 |
| reqwest | 0.12.28 | MIT OR Apache-2.0 | 2 |
| resvg | 0.46.0 | Apache-2.0 OR MIT | 2 |
| rfc6979 | 0.6.0 | Apache-2.0 OR MIT | 2 |
| rgb | 0.8.53 | MIT | 1 |
| ring | 0.17.14 | Apache-2.0 AND ISC | 6 |
| ropey | 1.6.1 | MIT | 1 |
| roxmltree | 0.21.2 | MIT OR Apache-2.0 | 2 |
| rsa | 0.10.0-rc.19 | MIT OR Apache-2.0 | 2 |
| rust-i18n | 4.2.4 | MIT | 1 |
| rust-i18n-macro | 4.2.4 | MIT | 0 |
| rust-i18n-support | 4.2.4 | MIT | 0 |
| rustc-hash | 1.1.0 | Apache-2.0/MIT | 2 |
| rustc-hash | 2.1.3 | Apache-2.0 OR MIT | 2 |
| rustc_version | 0.4.1 | MIT OR Apache-2.0 | 2 |
| rusticata-macros | 4.1.0 | MIT/Apache-2.0 | 2 |
| rustix | 1.1.6 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 4 |
| rustls | 0.23.46 | Apache-2.0 OR ISC OR MIT | 3 |
| rustls-native-certs | 0.8.5 | Apache-2.0 OR ISC OR MIT | 4 |
| rustls-pemfile | 2.2.0 | Apache-2.0 OR ISC OR MIT | 4 |
| rustls-pki-types | 1.16.0 | MIT OR Apache-2.0 | 2 |
| rustls-platform-verifier | 0.7.1 | MIT OR Apache-2.0 | 2 |
| rustls-webpki | 0.103.17 | ISC | 1 |
| rustversion | 1.0.23 | MIT OR Apache-2.0 | 2 |
| rustybuzz | 0.20.1 | MIT | 2 |
| ryu | 1.0.23 | Apache-2.0 OR BSL-1.0 | 2 |
| same-file | 1.0.6 | Unlicense/MIT | 3 |
| scopeguard | 1.2.0 | MIT OR Apache-2.0 | 2 |
| sec1 | 0.8.1 | Apache-2.0 OR MIT | 2 |
| security-framework | 3.7.0 | MIT OR Apache-2.0 | 2 |
| security-framework-sys | 2.17.0 | MIT OR Apache-2.0 | 2 |
| semver | 1.0.28 | MIT OR Apache-2.0 | 2 |
| serde | 1.0.229 | MIT OR Apache-2.0 | 2 |
| serde-saphyr | 1.3.0 | MIT OR Apache-2.0 | 2 |
| serde_core | 1.0.229 | MIT OR Apache-2.0 | 2 |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 | 2 |
| serde_json | 1.0.154 | MIT OR Apache-2.0 | 2 |
| serde_spanned | 0.6.9 | MIT OR Apache-2.0 | 2 |
| serde_urlencoded | 0.7.1 | MIT/Apache-2.0 | 2 |
| serdect | 0.4.3 | Apache-2.0 OR MIT | 2 |
| sha1 | 0.10.7 | MIT OR Apache-2.0 | 2 |
| sha1 | 0.11.0 | MIT OR Apache-2.0 | 2 |
| sha2 | 0.10.9 | MIT OR Apache-2.0 | 2 |
| sha2 | 0.11.1 | MIT OR Apache-2.0 | 2 |
| sha3 | 0.12.0 | MIT OR Apache-2.0 | 2 |
| shake | 0.1.0 | MIT OR Apache-2.0 | 2 |
| shlex | 1.3.0 | MIT OR Apache-2.0 | 2 |
| shlex | 2.0.1 | MIT OR Apache-2.0 | 2 |
| signal-hook-registry | 1.4.8 | MIT OR Apache-2.0 | 2 |
| signature | 2.2.0 | Apache-2.0 OR MIT | 2 |
| signature | 3.0.0 | Apache-2.0 OR MIT | 2 |
| simd-adler32 | 0.3.10 | MIT | 1 |
| simd-json | 0.18.1 | Apache-2.0 OR MIT | 2 |
| simdutf8 | 0.1.5 | MIT OR Apache-2.0 | 2 |
| simplecss | 0.2.2 | Apache-2.0 OR MIT | 3 |
| siphasher | 1.0.4 | MIT OR Apache-2.0 | 3 |
| slab | 0.4.12 | MIT | 1 |
| slotmap | 1.1.1 | Zlib | 1 |
| smallvec | 1.16.6 | MIT OR Apache-2.0 | 2 |
| socket2 | 0.6.5 | MIT OR Apache-2.0 | 2 |
| spirv | 0.4.0+sdk-1.4.341.0 | Apache-2.0 | 0 |
| spki | 0.7.3 | Apache-2.0 OR MIT | 2 |
| spki | 0.8.1 | Apache-2.0 OR MIT | 2 |
| sponge-cursor | 0.1.0 | MIT OR Apache-2.0 | 2 |
| stable_deref_trait | 1.2.1 | MIT OR Apache-2.0 | 2 |
| static_assertions | 1.1.0 | MIT OR Apache-2.0 | 2 |
| str_indices | 0.4.4 | MIT OR Apache-2.0 | 2 |
| strict-num | 0.1.1 | MIT | 1 |
| string_cache | 0.8.9 | MIT OR Apache-2.0 | 2 |
| string_cache_codegen | 0.5.4 | MIT OR Apache-2.0 | 2 |
| subtle | 2.6.1 | BSD-3-Clause | 1 |
| svgtypes | 0.16.2 | Apache-2.0 OR MIT | 3 |
| syn | 2.0.119 | MIT OR Apache-2.0 | 2 |
| syn | 3.0.10 | MIT OR Apache-2.0 | 2 |
| sync_wrapper | 1.0.2 | Apache-2.0 | 1 |
| synstructure | 0.13.2 | MIT | 1 |
| synstructure | 0.14.0 | MIT | 1 |
| syntect | 5.3.0 | MIT | 1 |
| taffy | 0.15.1 | MIT | 0 |
| tar | 0.4.46 | MIT OR Apache-2.0 | 2 |
| tendril | 0.4.3 | MIT/Apache-2.0 | 2 |
| thiserror | 1.0.69 | MIT OR Apache-2.0 | 2 |
| thiserror | 2.0.21 | MIT OR Apache-2.0 | 2 |
| thiserror-impl | 1.0.69 | MIT OR Apache-2.0 | 2 |
| thiserror-impl | 2.0.21 | MIT OR Apache-2.0 | 2 |
| time | 0.3.55 | MIT OR Apache-2.0 | 2 |
| time-core | 0.1.9 | MIT OR Apache-2.0 | 2 |
| time-macros | 0.2.32 | MIT OR Apache-2.0 | 2 |
| tiny-skia | 0.11.4 | BSD-3-Clause | 1 |
| tiny-skia-path | 0.11.4 | BSD-3-Clause | 1 |
| tinystr | 0.8.5 | Unicode-3.0 | 1 |
| tinyvec | 1.13.3 | Zlib OR Apache-2.0 OR MIT | 3 |
| tokio | 1.54.1 | MIT | 1 |
| tokio-macros | 2.7.2 | MIT | 1 |
| tokio-rustls | 0.26.6 | MIT OR Apache-2.0 | 2 |
| tokio-tungstenite | 0.28.0 | MIT | 1 |
| tokio-util | 0.7.20 | MIT | 1 |
| toml | 0.8.23 | MIT OR Apache-2.0 | 2 |
| toml_datetime | 0.6.11 | MIT OR Apache-2.0 | 2 |
| toml_datetime | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 | 2 |
| toml_edit | 0.22.27 | MIT OR Apache-2.0 | 2 |
| toml_edit | 0.25.19+spec-1.1.0 | MIT OR Apache-2.0 | 2 |
| toml_parser | 1.1.5+spec-1.1.0 | MIT OR Apache-2.0 | 2 |
| toml_write | 0.1.2 | MIT OR Apache-2.0 | 2 |
| tower | 0.5.3 | MIT | 1 |
| tower-http | 0.6.11 | MIT | 1 |
| tower-layer | 0.3.3 | MIT | 1 |
| tower-service | 0.3.3 | MIT | 1 |
| tracing | 0.1.44 | MIT | 1 |
| tracing-core | 0.1.36 | MIT | 2 |
| triomphe | 0.1.17 | MIT OR Apache-2.0 | 2 |
| try-lock | 0.2.5 | MIT | 1 |
| ttf-parser | 0.25.1 | MIT OR Apache-2.0 | 2 |
| tungstenite | 0.28.0 | MIT OR Apache-2.0 | 2 |
| two-face | 0.4.5 | MIT OR Apache-2.0 | 2 |
| typed-path | 0.13.0 | MIT OR Apache-2.0 | 2 |
| typenum | 1.20.1 | MIT OR Apache-2.0 | 3 |
| unicode-bidi | 0.3.18 | MIT OR Apache-2.0 | 4 |
| unicode-bidi-mirroring | 0.4.0 | MIT/Apache-2.0 | 2 |
| unicode-ccc | 0.4.0 | MIT/Apache-2.0 | 2 |
| unicode-id | 0.3.7 | MIT OR Apache-2.0 | 3 |
| unicode-ident | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 | 3 |
| unicode-linebreak | 0.1.5 | Apache-2.0 | 1 |
| unicode-normalization | 0.1.26 | (MIT OR Apache-2.0) AND Unicode-3.0 | 4 |
| unicode-properties | 0.1.5 | (MIT OR Apache-2.0) AND Unicode-3.0 | 4 |
| unicode-script | 0.5.9 | MIT OR Apache-2.0 | 2 |
| unicode-segmentation | 1.13.3 | MIT OR Apache-2.0 | 3 |
| unicode-vo | 0.1.0 | MIT/Apache-2.0 | 2 |
| unicode-width | 0.2.3 | MIT OR Apache-2.0 | 3 |
| universal-hash | 0.6.1 | MIT OR Apache-2.0 | 2 |
| untrusted | 0.9.0 | ISC | 1 |
| ureq | 3.5.2 | MIT OR Apache-2.0 | 2 |
| ureq-proto | 0.7.1 | MIT OR Apache-2.0 | 2 |
| url | 2.5.8 | MIT OR Apache-2.0 | 2 |
| usvg | 0.46.0 | Apache-2.0 OR MIT | 2 |
| utf-8 | 0.7.6 | MIT OR Apache-2.0 | 2 |
| utf8-zero | 0.8.1 | MIT OR Apache-2.0 | 2 |
| utf8_iter | 1.1.0 | Apache-2.0 OR MIT | 3 |
| utf8parse | 0.2.2 | Apache-2.0 OR MIT | 2 |
| utf_types | 1.0.0 | Apache-2.0 OR MIT | 3 |
| value-trait | 0.12.3 | Apache-2.0/MIT | 2 |
| version_check | 0.9.5 | MIT/Apache-2.0 | 2 |
| vsimd | 0.8.0 | MIT | 0 |
| vte | 0.15.0 | Apache-2.0 OR MIT | 2 |
| walkdir | 2.5.0 | Unlicense/MIT | 3 |
| want | 0.3.2 | MIT | 1 |
| wayland-sys | 0.31.11 | MIT | 1 |
| webpki-roots | 0.26.11 | CDLA-Permissive-2.0 | 1 |
| webpki-roots | 1.0.12 | CDLA-Permissive-2.0 | 1 |
| weezl | 0.1.12 | MIT OR Apache-2.0 | 2 |
| wgpu | 29.0.4 | MIT OR Apache-2.0 | 2 |
| wgpu-core | 29.0.4 | MIT OR Apache-2.0 | 2 |
| wgpu-core-deps-apple | 29.0.4 | MIT OR Apache-2.0 | 2 |
| wgpu-hal | 29.0.4 | MIT OR Apache-2.0 | 2 |
| wgpu-naga-bridge | 29.0.4 | MIT OR Apache-2.0 | 2 |
| wgpu-types | 29.0.4 | MIT OR Apache-2.0 | 2 |
| winnow | 0.7.15 | MIT | 1 |
| winnow | 1.0.5 | MIT | 1 |
| wnaf | 0.14.1 | Apache-2.0 OR MIT | 2 |
| writeable | 0.6.4 | Unicode-3.0 | 1 |
| wuff | 0.2.10 | MIT | 0 |
| x25519-dalek | 3.0.0 | BSD-3-Clause | 1 |
| x509-parser | 0.17.0 | MIT OR Apache-2.0 | 2 |
| x509-parser | 0.18.1 | MIT OR Apache-2.0 | 2 |
| xattr | 1.7.0 | MIT OR Apache-2.0 | 2 |
| xml5ever | 0.18.1 | MIT OR Apache-2.0 | 2 |
| xmlwriter | 0.1.0 | MIT | 1 |
| yasna | 0.6.0 | MIT OR Apache-2.0 | 2 |
| yoke | 0.8.4 | Unicode-3.0 | 1 |
| yoke-derive | 0.8.4 | Unicode-3.0 | 1 |
| zerocopy | 0.8.63 | BSD-2-Clause OR Apache-2.0 OR MIT | 3 |
| zerocopy-derive | 0.8.63 | BSD-2-Clause OR Apache-2.0 OR MIT | 3 |
| zerofrom | 0.1.8 | Unicode-3.0 | 1 |
| zerofrom-derive | 0.1.8 | Unicode-3.0 | 1 |
| zeroize | 1.9.2 | Apache-2.0 OR MIT | 2 |
| zeroize_derive | 1.5.0 | Apache-2.0 OR MIT | 2 |
| zerotrie | 0.2.5 | Unicode-3.0 | 1 |
| zerovec | 0.11.9 | Unicode-3.0 | 1 |
| zerovec-derive | 0.11.7 | Unicode-3.0 | 1 |
| zip | 9.8.1 | MIT | 1 |
| zlib-rs | 0.6.8 | Zlib | 1 |
| zmij | 1.0.23 | MIT | 1 |
| zopfli | 0.8.4 | Apache-2.0 | 1 |
| zstd | 0.14.4 | BSD-3-Clause | 1 |
| zstd-safe | 8.3.0 | BSD-3-Clause | 1 |
| zstd-sys | 2.1.3+zstd.1.5.7 | BSD-3-Clause AND BSD-2-Clause | 5 |
| zune-core | 0.5.3 | MIT OR Apache-2.0 OR Zlib | 3 |
| zune-jpeg | 0.5.15 | MIT OR Apache-2.0 OR Zlib | 3 |
| cc | 1.11.3 | MIT OR Apache-2.0 | 2 |

| Asset | Upstream license snapshot | License |
| --- | --- | --- |
| Lucide icons | [e715245d6266](https://raw.githubusercontent.com/lucide-icons/lucide/e715245d62667c800e7f54c94b1b023692e900a3/LICENSE) | [notice](third-party-assets/lucide/LICENSE) |
