#pragma once
// HID uses one outstanding packet per endpoint. Never send a trailing ZLP or
// wait for the second bank: a full 64-byte interrupt packet is already complete.
bool USB_PacketComplete(u8 ep) {
    if (!_usbConfiguration || (_usbSuspendState & (1 << SUSPI)))
        return false;
    LockEP lock(ep);
    return !(UESTA0X & ((1 << NBUSYBK0) | (1 << NBUSYBK1))) && !FifoByteCount();
}
bool USB_TrySendPacket(u8 ep, const u8 *data, u8 length) {
    if (!_usbConfiguration || (_usbSuspendState & (1 << SUSPI)) || !length || length > 64)
        return false;
    LockEP lock(ep);
    if ((UESTA0X & ((1 << NBUSYBK0) | (1 << NBUSYBK1))) || FifoByteCount() || !ReadWriteAllowed())
        return false;
    for (u8 i = 0; i < length; ++i)
        Send8(data[i]);
    ReleaseTX();
    return true;
}
