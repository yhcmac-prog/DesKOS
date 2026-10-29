#!/bin/sh
# build.sh - build tAbLeOS / DesKOS disk images.
# Usage: BRAND=table|desk|both EDITION=server|workstation|both ./build.sh
#        (defaults: BRAND=both EDITION=both — 4 images total)
set -eu
cd "$(dirname "$0")"

export PATH="$HOME/.cargo/bin:$PATH"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"

ZIG=/Users/macbook/Desktop/os/e1LibreOS/downloads/zig/zig
mkdir -p build

echo "== [1/6] boot sectors =="
$ZIG cc -target x86_64-freestanding-none -nostdlib -static \
    -Wl,-T,boot/bootsect.ld boot/bootsect.S -o build/bootsect.elf
$ZIG objcopy -O binary build/bootsect.elf build/bootsect.bin
$ZIG cc -target x86_64-freestanding-none -nostdlib -static \
    -Wl,-T,boot/stage2.ld boot/stage2.S -o build/stage2.elf
$ZIG objcopy -O binary build/stage2.elf build/stage2.bin

echo "== [2/6] busybox .rit package (e1wine payload) =="
BUSYBOX=/Users/macbook/Desktop/os/e1LibreOS/downloads/busybox-x86_64
if [ -f "$BUSYBOX" ]; then
    python3 tools/mkrit.py busybox build/busybox.rit "$BUSYBOX"
else
    echo "warning: $BUSYBOX missing, skip .rit package"
fi

echo "== [3/6] userspace (single build, edition=workstation for full set) =="
# Userspace ELFs are edition-agnostic: e1wm/e1apfs only differ by whether
# they're included in the e1fss image. We always build the full set here.
for BRAND_ARG in ${BRAND:-both}; do
    case "$BRAND_ARG" in
        both) inner_brands="table desk" ;;
        table|desk) inner_brands="$BRAND_ARG" ;;
        *) echo "unknown BRAND: $BRAND_ARG"; exit 1 ;;
    esac
    for b in $inner_brands; do
        ( cd userspace/hello && cargo rustc -- --cfg "brand=\"$b\"" )
        ( cd apps/e1wm && cargo rustc -- --cfg "brand=\"$b\"" )
        ( cd apps/exaltop && cargo rustc -- --cfg "brand=\"$b\"" )
        ( cd apps/e1dm && cargo rustc -- --cfg "brand=\"$b\"" )
        ( cd apps/e1vfsx && cargo rustc )
        ( cd apps/e1apfs && cargo rustc )
        cp userspace/target/x86_64-unknown-none/debug/hello build/hello
        cp apps/target/x86_64-unknown-none/debug/e1wm build/e1wm
        cp apps/target/x86_64-unknown-none/debug/exaltop build/exaltop
        cp apps/target/x86_64-unknown-none/debug/e1dm build/e1dm
        cp apps/target/x86_64-unknown-none/debug/e1vfsx build/e1vfsx
        cp apps/target/x86_64-unknown-none/debug/e1apfs build/e1apfs
    done
done

build_brand_edition() {
    BRAND="$1"
    EDITION="$2"
    case "$BRAND" in
        table) NAME=tableos ;;
        desk)  NAME=deskos ;;
        *) echo "unknown BRAND: $BRAND"; exit 1 ;;
    esac
    case "$EDITION" in
        server) ED_NAME="-server" ;;
        workstation) ED_NAME="-workstation" ;;
        *) echo "unknown EDITION: $EDITION"; exit 1 ;;
    esac

    echo "== [4/6] kernel ($BRAND / $EDITION) =="
    ( cd kernel
      RUSTFLAGS="--cfg brand=\"$BRAND\" --cfg edition=\"$EDITION\" --cfg has_sched -C relocation-model=static -C link-arg=-no-pie -C link-arg=-Tlinker.ld" cargo build
    )
    $ZIG objcopy -O binary \
        kernel/target/x86_64-unknown-none/debug/tableos-kernel build/kernel.bin

    echo "== [5/6] ush ($BRAND / $EDITION) =="
    ( cd userspace/ush && cargo rustc -- --cfg "brand=\"$BRAND\"" --cfg "edition=\"$EDITION\"" )
    cp userspace/target/x86_64-unknown-none/debug/ush build/ush

    echo "== [6/6] e1fss + disk image ($BRAND / $EDITION) =="
    python3 tools/mke1fss.py build/e1fss.img "$BRAND" "$EDITION"
    OUT="build/${NAME}${ED_NAME}-0.1-amd64.img"
    python3 tools/mkimage.py "$OUT" 16384 build/e1fss.img
    echo "build OK: $OUT"
}

BRAND="${BRAND:-both}"
EDITION="${EDITION:-both}"

# expand to lists
case "$BRAND" in
    both) brands="table desk" ;;
    table|desk) brands="$BRAND" ;;
    *) echo "unknown BRAND: $BRAND (use table|desk|both)"; exit 1 ;;
esac
case "$EDITION" in
    both) editions="server workstation" ;;
    server|workstation) editions="$EDITION" ;;
    *) echo "unknown EDITION: $EDITION (use server|workstation|both)"; exit 1 ;;
esac

for b in $brands; do
    for e in $editions; do
        build_brand_edition "$b" "$e"
    done
done
