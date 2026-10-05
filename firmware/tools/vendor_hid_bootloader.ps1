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
using System.Text;
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
    private const byte CommandReportId = 10;
    private const byte ResponseReportId = 11;
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
                caps.InputReportByteLength < 2 ||
                caps.OutputReportByteLength < 2)
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
                    return device;
                }
            }
        }
        finally
        {
            SetupDiDestroyDeviceInfoList(infoSet);
        }

        throw new InvalidOperationException("Vendor HID device not found.");
    }

    public static string SendCommand(ushort vid, ushort pid, ushort usagePage, ushort usage, string command, int timeoutMs)
    {
        HidDevice device = Find(vid, pid, usagePage, usage);
        using (SafeFileHandle handle = Open(device.Path))
        using (FileStream stream = new FileStream(handle, FileAccess.ReadWrite, Math.Max(device.InputReportLength, device.OutputReportLength), true))
        {
            byte[] output = new byte[device.OutputReportLength];
            byte[] commandBytes = Encoding.ASCII.GetBytes(command + "\n");
            if (commandBytes.Length > output.Length - 1)
            {
                throw new InvalidOperationException("Command too long for HID report.");
            }

            output[0] = CommandReportId;
            Array.Copy(commandBytes, 0, output, 1, commandBytes.Length);
            stream.Write(output, 0, output.Length);
            stream.Flush();

            StringBuilder text = new StringBuilder();
            DateTime deadline = DateTime.UtcNow.AddMilliseconds(timeoutMs);
            while (DateTime.UtcNow < deadline)
            {
                byte[] input = new byte[device.InputReportLength];
                Task<int> readTask = stream.ReadAsync(input, 0, input.Length);
                int remaining = Math.Max(1, (int)(deadline - DateTime.UtcNow).TotalMilliseconds);
                if (!readTask.Wait(remaining))
                {
                    break;
                }

                int read = readTask.Result;
                if (read <= 0 || input[0] != ResponseReportId)
                {
                    continue;
                }

                for (int i = 1; i < read; i++)
                {
                    byte value = input[i];
                    if (value == 0)
                    {
                        continue;
                    }
                    if (value == (byte)'\n')
                    {
                        string line = text.ToString().TrimEnd('\r');
                        if (line.StartsWith("mscv-keyboard:ready", StringComparison.Ordinal))
                        {
                            text.Clear();
                            continue;
                        }
                        return line;
                    }
                    text.Append((char)value);
                }
            }
        }

        throw new TimeoutException("No Vendor HID response.");
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
