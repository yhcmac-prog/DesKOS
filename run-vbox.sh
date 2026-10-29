#!/bin/sh
# run-vbox.sh - boot tAbLeOS image in VirtualBox.
# Usage: ./run-vbox.sh          start the VM
#        ./run-vbox.sh clean    unregister and delete
# Env:   E1_IMG=<path>, E1_HEADLESS=1
set -eu
cd "$(dirname "$0")"

IMG=${E1_IMG:-$PWD/build/tableos-0.1-amd64.img}
VM=tAbLeOS
VB=VBoxManage
VDI="$PWD/.vbox/tableos.vdi"

command -v $VB >/dev/null 2>&1 || { echo "ERROR: VirtualBox not installed"; exit 1; }

if [ "${1:-run}" = "clean" ]; then
    $VB controlvm "$VM" poweroff 2>/dev/null || true
    $VB unregistervm "$VM" --delete 2>/dev/null || true
    echo "Cleaned VM: $VM"
    exit 0
fi

[ -f "$IMG" ] || { echo "ERROR: $IMG not found, run ./build.sh first"; exit 1; }

mkdir -p "$PWD/.vbox"
: > "$PWD/.vbox/serial.log"

if ! $VB list vms | grep -q "\"$VM\""; then
    $VB createvm --name "$VM" --basefolder "$PWD/.vbox" --register
    if [ "${E1_UART_TCP:-0}" = "1" ]; then
        $VB modifyvm "$VM" --ostype Other_64 --memory 256 \
            --audio none --usb off --graphicscontroller vboxvga --boot1 disk \
            --nic1 nat --nictype1 82540EM --cableconnected1 on \
            --uart1 0x3F8 4 --uart-mode1 tcpserver 4555
    else
        $VB modifyvm "$VM" --ostype Other_64 --memory 256 \
            --audio none --usb off --graphicscontroller vboxvga --boot1 disk \
            --nic1 nat --nictype1 82540EM --cableconnected1 on \
            --uart1 0x3F8 4 --uart-mode1 file "$PWD/.vbox/serial.log"
    fi
    $VB storagectl "$VM" --name IDE --add ide --controller PIIX4
else
    # mode may differ between runs: apply it every time
    if [ "${E1_UART_TCP:-0}" = "1" ]; then
        $VB modifyvm "$VM" --uart-mode1 tcpserver 4555 2>/dev/null || true
    else
        $VB modifyvm "$VM" --uart-mode1 file "$PWD/.vbox/serial.log" 2>/dev/null || true
    fi
fi

# (Re)create VDI from the raw image each run unless E1_KEEP=1 (persist test)
if [ "${E1_KEEP:-0}" = "1" ] && [ -f "$VDI" ]; then
    echo "keep existing VDI (E1_KEEP=1)"
else
    $VB storageattach "$VM" --storagectl IDE --port 0 --device 0 --medium none 2>/dev/null || true
    $VB closemedium disk "$VDI" 2>/dev/null || true
    rm -f "$VDI"
    $VB convertfromraw "$IMG" "$VDI" --format VDI
fi
$VB storageattach "$VM" --storagectl IDE --port 0 --device 0 \
    --type hdd --medium "$VDI"

if [ "${E1_HEADLESS:-0}" = "1" ]; then
    $VB startvm "$VM" --type headless
else
    $VB startvm "$VM" --type gui
fi
