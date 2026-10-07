[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new()
$OutputEncoding = [System.Text.UTF8Encoding]::new()
chcp 65001 > $null

function Add-VendorHidUploadType {
    if ("VendorHidUpload" -as [type]) {
        return
    }

    Add-Type -TypeDefinition @'
using Microsoft.Win32.SafeHandles;
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Diagnostics;
using System.Security.Cryptography;
using System.Threading.Tasks;

public static class VendorHidUpload
{
    private const int DIGCF_PRESENT = 0x00000002;
    private const int DIGCF_DEVICEINTERFACE = 0x00000010;
    private const uint GENERIC_READ = 0x80000000;
    private const uint GENERIC_WRITE = 0x40000000;
    private const uint FILE_SHARE_READ = 0x00000001;
    private const uint FILE_SHARE_WRITE = 0x00000002;
    private const uint OPEN_EXISTING = 3;
    private const uint FILE_ATTRIBUTE_NORMAL = 0x00000080;
    private const uint FILE_FLAG_OVERLAPPED = 0x40000000;
    private const byte CommandReportId = 12;
    private const byte ResponseReportId = 13;
    private const int HIDP_STATUS_SUCCESS = 0x00110000;

    [StructLayout(LayoutKind.Sequential)]
    private struct SP_DEVICE_INTERFACE_DATA
    {
        public int cbSize;
        public Guid InterfaceClassGuid;
        public int Flags;
        public IntPtr Reserved;
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct SP_DEVICE_INTERFACE_DETAIL_DATA
    {
        public int cbSize;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 1024)]
        public string DevicePath;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct HIDD_ATTRIBUTES
    {
        public int Size;
        public ushort VendorID;
        public ushort ProductID;
        public ushort VersionNumber;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct HIDP_CAPS
    {
        public ushort Usage;
        public ushort UsagePage;
        public ushort InputReportByteLength;
        public ushort OutputReportByteLength;
        public ushort FeatureReportByteLength;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 17)]
        public ushort[] Reserved;
        public ushort NumberLinkCollectionNodes;
        public ushort NumberInputButtonCaps;
        public ushort NumberInputValueCaps;
        public ushort NumberInputDataIndices;
        public ushort NumberOutputButtonCaps;
        public ushort NumberOutputValueCaps;
        public ushort NumberOutputDataIndices;
        public ushort NumberFeatureButtonCaps;
        public ushort NumberFeatureValueCaps;
        public ushort NumberFeatureDataIndices;
    }

    private sealed class HidDevice
    {
        public string Path = "";
        public ushort InputReportLength;
        public ushort OutputReportLength;
    }

    [DllImport("hid.dll")]
    private static extern void HidD_GetHidGuid(out Guid hidGuid);

    [DllImport("hid.dll", SetLastError = true)]
    private static extern bool HidD_GetFeature(SafeFileHandle handle, [In, Out] byte[] report, int length);

    [DllImport("hid.dll", SetLastError = true)]
    private static extern bool HidD_GetAttributes(SafeFileHandle hidDeviceObject, ref HIDD_ATTRIBUTES attributes);

    [DllImport("hid.dll", SetLastError = true)]
    private static extern bool HidD_GetPreparsedData(SafeFileHandle hidDeviceObject, out IntPtr preparsedData);

    [DllImport("hid.dll", SetLastError = true)]
    private static extern bool HidD_FreePreparsedData(IntPtr preparsedData);

    [DllImport("hid.dll")]
    private static extern int HidP_GetCaps(IntPtr preparsedData, out HIDP_CAPS capabilities);

    [DllImport("setupapi.dll", SetLastError = true)]
    private static extern IntPtr SetupDiGetClassDevs(ref Guid classGuid, IntPtr enumerator, IntPtr hwndParent, int flags);

    [DllImport("setupapi.dll", SetLastError = true)]
    private static extern bool SetupDiEnumDeviceInterfaces(IntPtr deviceInfoSet, IntPtr deviceInfoData, ref Guid interfaceClassGuid, uint memberIndex, ref SP_DEVICE_INTERFACE_DATA deviceInterfaceData);

    [DllImport("setupapi.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    private static extern bool SetupDiGetDeviceInterfaceDetail(IntPtr deviceInfoSet, ref SP_DEVICE_INTERFACE_DATA deviceInterfaceData, ref SP_DEVICE_INTERFACE_DETAIL_DATA deviceInterfaceDetailData, int deviceInterfaceDetailDataSize, IntPtr requiredSize, IntPtr deviceInfoData);

    [DllImport("setupapi.dll", SetLastError = true)]
    private static extern bool SetupDiDestroyDeviceInfoList(IntPtr deviceInfoSet);

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    private static extern SafeFileHandle CreateFile(string fileName, uint desiredAccess, uint shareMode, IntPtr securityAttributes, uint creationDisposition, uint flagsAndAttributes, IntPtr templateFile);

    private static SafeFileHandle Open(string path)
    {
        return CreateFile(
            path,
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            IntPtr.Zero,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
            IntPtr.Zero);
    }

    private static bool Query(string path, ushort vid, ushort pid, ushort usagePage, ushort usage, out HidDevice device)
    {
        device = null;
        using (SafeFileHandle handle = Open(path))
        {
            if (handle.IsInvalid)
            {
                return false;
            }

            HIDD_ATTRIBUTES attributes = new HIDD_ATTRIBUTES();
            attributes.Size = Marshal.SizeOf(typeof(HIDD_ATTRIBUTES));
            if (!HidD_GetAttributes(handle, ref attributes) ||
                attributes.VendorID != vid ||
                attributes.ProductID != pid)
            {
                return false;
            }

            IntPtr preparsedData;
            if (!HidD_GetPreparsedData(handle, out preparsedData))
            {
                return false;
            }

            HIDP_CAPS caps;
            int status = HidP_GetCaps(preparsedData, out caps);
            HidD_FreePreparsedData(preparsedData);

            if (status != HIDP_STATUS_SUCCESS ||
                caps.UsagePage != usagePage ||
                caps.Usage != usage ||
                caps.InputReportByteLength != 64 ||
                caps.OutputReportByteLength != 64 || caps.FeatureReportByteLength != 64)
            {
                return false;
            }

            device = new HidDevice();
            device.Path = path;
            device.InputReportLength = caps.InputReportByteLength;
            device.OutputReportLength = caps.OutputReportByteLength;
            return true;
        }
    }

    private static HidDevice Find(ushort vid, ushort pid, ushort usagePage, ushort usage)
    {
        Guid hidGuid;
        HidD_GetHidGuid(out hidGuid);
        IntPtr infoSet = SetupDiGetClassDevs(ref hidGuid, IntPtr.Zero, IntPtr.Zero, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE);
        if (infoSet == IntPtr.Zero || infoSet.ToInt64() == -1)
        {
            throw new InvalidOperationException("SetupDiGetClassDevs failed.");
        }

        HidDevice found = null;
        try
        {
            for (uint index = 0; ; index++)
            {
                SP_DEVICE_INTERFACE_DATA interfaceData = new SP_DEVICE_INTERFACE_DATA();
                interfaceData.cbSize = Marshal.SizeOf(typeof(SP_DEVICE_INTERFACE_DATA));
                if (!SetupDiEnumDeviceInterfaces(infoSet, IntPtr.Zero, ref hidGuid, index, ref interfaceData))
                {
                    break;
                }

                SP_DEVICE_INTERFACE_DETAIL_DATA detailData = new SP_DEVICE_INTERFACE_DETAIL_DATA();
                detailData.cbSize = IntPtr.Size == 8 ? 8 : 6;
                int detailSize = Marshal.SizeOf(typeof(SP_DEVICE_INTERFACE_DETAIL_DATA));
                if (!SetupDiGetDeviceInterfaceDetail(infoSet, ref interfaceData, ref detailData, detailSize, IntPtr.Zero, IntPtr.Zero))
                {
                    continue;
                }

                HidDevice device;
                if (Query(detailData.DevicePath, vid, pid, usagePage, usage, out device))
                {
                    if (found != null) throw new InvalidOperationException("Multiple Vendor HID devices; connect only the board to flash.");
                    found = device;
                }
            }
        }
        finally
        {
            SetupDiDestroyDeviceInfoList(infoSet);
        }

        if (found != null) return found;
        throw new InvalidOperationException("Vendor HID device not found.");
    }

    private static uint Read32(byte[] data, int offset)
    {
        return (uint)data[offset] | ((uint)data[offset+1] << 8) |
               ((uint)data[offset+2] << 16) | ((uint)data[offset+3] << 24);
    }
    private static void Write32(byte[] data, int offset, uint value)
    {
        for (int i=0; i<4; i++) data[offset+i] = (byte)(value >> (8*i));
    }
    private static void Control(Stream stream, byte opcode, uint session, uint sequence,
                                Stopwatch timer, int timeoutMs)
    {
        byte[] output = new byte[64];
        output[0] = CommandReportId; output[1] = 3; output[2] = opcode;
        Write32(output, 5, session); Write32(output, 9, sequence);
        int remaining = timeoutMs - (int)timer.ElapsedMilliseconds;
        if (remaining <= 0 || !stream.WriteAsync(output, 0, output.Length).Wait(remaining))
            throw new TimeoutException("Vendor HID v3 write timed out.");
        while (true)
        {
            byte[] input = new byte[64];
            remaining = timeoutMs - (int)timer.ElapsedMilliseconds;
            if (remaining <= 0) throw new TimeoutException("No Vendor HID v3 response.");
            Task<int> read = stream.ReadAsync(input, 0, input.Length);
            if (!read.Wait(remaining)) throw new TimeoutException("No Vendor HID v3 response.");
            if (read.Result != 64 || input[0] != ResponseReportId || input[1] != 3 ||
                input[3] > 51 || input[4] != 0)
                throw new InvalidOperationException("Invalid Vendor HID v3 response framing.");
            for (int i=13+input[3]; i<64; i++)
                if (input[i] != 0) throw new InvalidOperationException("Invalid v3 response padding.");
            if (Read32(input, 5) != session) continue;
            if (input[2] == 127) throw new InvalidOperationException("Device fault in bootloader session.");
            if (input[2] != (opcode | 128) || Read32(input, 9) != sequence || input[3] != 1)
                throw new InvalidOperationException("Unexpected Vendor HID v3 control response.");
            if (input[13] != 0) throw new InvalidOperationException("Bootloader request rejected, v3 code=" + input[13]);
            return;
        }
    }
    public static string SendCommand(ushort vid, ushort pid, ushort usagePage, ushort usage, string command, int timeoutMs)
    {
        if (command != "enter_bootloader") throw new ArgumentException("Only the v3 bootloader operation is supported.");
        HidDevice device = Find(vid, pid, usagePage, usage);
        using (SafeFileHandle handle = Open(device.Path))
        {
            if (handle.IsInvalid) throw new IOException("Cannot open Vendor HID device.");
            byte[] feature = new byte[64]; feature[0] = 14;
            if (!HidD_GetFeature(handle, feature, feature.Length) || feature[0] != 14 ||
                feature[1] != 'F' || feature[2] != 'M' || feature[3] != 'T' || feature[4] != '3' || feature[5] != 3)
                throw new InvalidOperationException("Unsupported device: Vendor HID v3 is required. Use physical reset/BOOTSEL for old firmware.");
            for (int i=57; i<64; i++)
                if (feature[i] != 0) throw new InvalidOperationException("Invalid v3 feature padding.");
            if ((feature[10] | feature[11] << 8) < 1000 || feature[14] == 0)
                throw new InvalidOperationException("Invalid v3 capabilities.");
            byte[] nonce = new byte[4];
            using (RandomNumberGenerator random = RandomNumberGenerator.Create()) random.GetBytes(nonce);
            uint session = Read32(nonce, 0); if (session == 0) session = 1;
            using (FileStream stream = new FileStream(handle, FileAccess.ReadWrite, 64, true))
            {
                Stopwatch timer = Stopwatch.StartNew();
                Control(stream, 1, session, 1, timer, timeoutMs);
                Control(stream, 6, session, 2, timer, timeoutMs);
            }
            return "ok:v3_bootloader";
        }
    }
}
'@
}

function Invoke-VendorHidCommand {
    param(
        [Parameter(Mandatory = $true)]
        [UInt16]$Vid,
        [Parameter(Mandatory = $true)]
        [UInt16]$ProductId,
        [Parameter(Mandatory = $true)]
        [UInt16]$UsagePage,
        [Parameter(Mandatory = $true)]
        [UInt16]$Usage,
        [Parameter(Mandatory = $true)]
        [string]$Command,
        [int]$TimeoutMs = 1500
    )

    Add-VendorHidUploadType
    return [VendorHidUpload]::SendCommand($Vid, $ProductId, $UsagePage, $Usage, $Command, $TimeoutMs)
}
