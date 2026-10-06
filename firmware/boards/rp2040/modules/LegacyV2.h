// RP2040 internal module; included once by Firmware.cpp.
struct CommandParser {
    char line_buffer[LineBufferSize];
    uint8_t line_length = 0;
    bool discarding_line = false;
    bool swallow_lf = false;
    unsigned long last_rx_byte_at = 0;
};

CommandParser vendor_parser;
unsigned long last_lease_renewed_at = 0;

uint16_t line_too_long_count = 0;
uint16_t invalid_byte_count = 0;
uint16_t line_timeout_count = 0;
uint16_t bad_report_id_count = 0;
uint16_t failsafe_count = 0;

bool bootloader_reset_pending = false;
unsigned long bootloader_reset_requested_at = 0;

void resetParser(CommandParser &parser) {
    parser.line_length = 0;
    parser.discarding_line = false;
    parser.swallow_lf = false;
}

char *trimAndLower(char *text) {
    while (*text == ' ') {
        ++text;
    }

    char *cursor = text;
    char *trimmed_end = text;

    while (*cursor != '\0') {
        if (*cursor >= 'A' && *cursor <= 'Z') {
            *cursor = static_cast<char>(*cursor - 'A' + 'a');
        }

        if (*cursor != ' ') {
            trimmed_end = cursor + 1;
        }

        ++cursor;
    }

    *trimmed_end = '\0';
    return text;
}

void renewLease() { last_lease_renewed_at = millis(); }

class TextBuilder {
  public:
    TextBuilder() : length_(0) {}

    void append(char value) {
        if (length_ < sizeof(buffer_)) {
            buffer_[length_++] = value;
        }
    }

    void append(const char *text) {
        while (*text != '\0') {
            append(*text++);
        }
    }

    void appendUnsigned(uint32_t value) {
        char temporary[10];
        uint8_t count = 0;

        do {
            temporary[count++] = static_cast<char>('0' + value % 10);
            value /= 10;
        } while (value != 0 && count < sizeof(temporary));

        while (count > 0) {
            append(temporary[--count]);
        }
    }

    void appendHexByte(uint8_t value) {
        static const char Hex[] = "0123456789ABCDEF";
        append(Hex[value >> 4]);
        append(Hex[value & 0x0F]);
    }

    const char *data() const { return buffer_; }

    uint8_t length() const { return length_; }

  private:
    char buffer_[127];
    uint8_t length_;
};

unsigned long leaseRemaining(unsigned long now) {
    if (!keyboard.hasPressedKeys() && mouse_report.buttons == 0) {
        return 0;
    }

    const unsigned long elapsed = now - last_lease_renewed_at;
    return elapsed >= FailsafeReleaseMs ? 0 : FailsafeReleaseMs - elapsed;
}

void queueHello() {
    TextBuilder output;
    output.append("ok:hello,protocol=2,fw=3.0.2,lease_ms=");
    output.appendUnsigned(FailsafeReleaseMs);
    output.append(",max_nonmod=6,mouse=1\n");
    queueRam(output.data(), output.length());
}

void queueStatus() {
    const KeyboardReport report = keyboard.report();
    uint8_t nonModifierCount = 0;
    for (uint8_t usage : report.keys) {
        if (usage != 0) {
            ++nonModifierCount;
        }
    }
    const uint8_t pressedCount =
        static_cast<uint8_t>(nonModifierCount + popcount8(report.modifiers));

    TextBuilder output;

    output.append("ok:status,p=");
    output.appendUnsigned(pressedCount);
    output.append(",n=");
    output.appendUnsigned(nonModifierCount);
    output.append(",m=");
    output.appendHexByte(report.modifiers);
    output.append(",k=");

    bool wrote_key = false;
    for (uint8_t usage : report.keys) {
        if (usage == 0) {
            continue;
        }

        if (wrote_key) {
            output.append('/');
        }

        output.appendHexByte(usage);
        wrote_key = true;
    }

    if (!wrote_key) {
        output.append('-');
    }

    output.append(",lease=");
    output.appendUnsigned(leaseRemaining(millis()));
    output.append(",fs=");
    output.appendUnsigned(failsafe_count);
    output.append(",rx=");
    output.appendUnsigned(static_cast<uint32_t>(line_too_long_count) + invalid_byte_count +
                          line_timeout_count + bad_report_id_count +
                          rx_overflow_count.load(std::memory_order_relaxed));
    output.append(",tx=");
    output.appendUnsigned(txDroppedMessages());
    output.append(",hid=");
    output.appendUnsigned(static_cast<uint32_t>(keyboard.sendFailures()) + mouse_send_failures);
    output.append(",mb=");
    output.appendHexByte(mouse_report.buttons);
    output.append('\n');

    queueRam(output.data(), output.length());
}

bool addKeyToSnapshot(KeyboardReport &report, const KeySpec &key) {
    if (key.modifier_mask != 0) {
        report.modifiers |= key.modifier_mask;
        return true;
    }

    for (uint8_t usage : report.keys) {
        if (usage == key.usage) {
            return true;
        }
    }

    for (uint8_t i = 0; i < MaxNonModifierKeys; ++i) {
        if (report.keys[i] == 0) {
            report.keys[i] = key.usage;
            return true;
        }
    }

    return false;
}

void respondToKeyResult(ApplyResult result, bool is_down) {
    switch (result) {
    case ApplyResult::Applied:
        renewLease();
        if (EnableKeyCommandAcks) {
            is_down ? QUEUE_TEXT("ok:down\n") : QUEUE_TEXT("ok:up\n");
        }
        return;

    case ApplyResult::AlreadyDesired:
        renewLease();
        if (EnableKeyCommandAcks) {
            is_down ? QUEUE_TEXT("ok:already_down\n") : QUEUE_TEXT("ok:already_up\n");
        }
        return;

    case ApplyResult::TransportFailed:
        QUEUE_ERROR("err:hid_send_failed\n");
        return;

    case ApplyResult::ReportFull:
        QUEUE_ERROR("err:hid_report_full\n");
        return;
    }
}

void handleSnapshotCommand(char *key_list) {
    // Parse separately so unknown tokens and malformed lists get useful errors.
    KeyboardReport desired{};
    char *cursor = trimAndLower(key_list);

    if (*cursor != '\0') {
        while (true) {
            char *separator = strchr(cursor, ',');
            if (separator != nullptr) {
                *separator = '\0';
            }

            char *token = trimAndLower(cursor);
            if (*token == '\0') {
                QUEUE_ERROR("err:bad_sync\n");
                return;
            }

            KeySpec key{};
            if (!parseKeyToken(token, key)) {
                QUEUE_ERROR("err:unknown_key\n");
                return;
            }

            if (!addKeyToSnapshot(desired, key)) {
                QUEUE_ERROR("err:hid_report_full\n");
                return;
            }

            if (separator == nullptr) {
                break;
            }

            cursor = separator + 1;
        }
    }

    const ApplyResult result = keyboard.applySnapshot(desired);

    renewLease();

    if (EnableKeyCommandAcks) {
        result == ApplyResult::Applied ? QUEUE_TEXT("ok:sync\n")
                                       : QUEUE_TEXT("ok:sync_unchanged\n");
    }
}

void handleReleaseAll() {
    const ApplyResult result = keyboard.releaseAll();
    protocol_owner = ProtocolOwner::None;
    const bool mouse_ok = releaseAllMouseButtons();

    (void)result;
    if (mouse_ok) {
        QUEUE_TEXT("ok:release_all\n");
    } else {
        QUEUE_ERROR("err:release_all_send_failed\n");
    }
}

bool parseSignedPair(const char *text, int32_t &first, int32_t &second) {
    char *end = nullptr;
    const long parsed_first = strtol(text, &end, 10);
    if (end == text || *end != ',') {
        return false;
    }

    const char *second_text = end + 1;
    const long parsed_second = strtol(second_text, &end, 10);
    if (end == second_text || *end != '\0') {
        return false;
    }

    first = static_cast<int32_t>(parsed_first);
    second = static_cast<int32_t>(parsed_second);
    return true;
}

bool handleMouseCommand(char *line) {
    int32_t first = 0;
    int32_t second = 0;

    if (strncmp(line, "mouse_move:", 11) == 0) {
        if (!parseSignedPair(line + 11, first, second) || first < -MaxMouseDeltaPerCommand ||
            first > MaxMouseDeltaPerCommand || second < -MaxMouseDeltaPerCommand ||
            second > MaxMouseDeltaPerCommand) {
            QUEUE_ERROR("err:bad_mouse\n");
            return true;
        }
        if (!sendMouseDelta(first, second, 0, 0)) {
            QUEUE_ERROR("err:hid_send_failed\n");
            return true;
        }
        renewLease();
        if (EnableKeyCommandAcks) {
            QUEUE_TEXT("ok:mouse_move\n");
        }
        return true;
    }

    // Never reinterpret an old absolute command as a relative delta.
    if (strncmp(line, "mouse_abs:", 10) == 0) {
        QUEUE_ERROR("err:unsupported_mouse_abs\n");
        return true;
    }

    if (strncmp(line, "mouse_button:", 13) == 0) {
        char *separator = strchr(line + 13, ',');
        if (separator == nullptr) {
            QUEUE_ERROR("err:bad_mouse\n");
            return true;
        }
        *separator = '\0';
        char *end = nullptr;
        const long button = strtol(line + 13, &end, 10);
        const bool down = strcmp(separator + 1, "down") == 0;
        const bool up = strcmp(separator + 1, "up") == 0;
        if (end == line + 13 || *end != '\0' || button < 1 || button > 5 || (!down && !up)) {
            QUEUE_ERROR("err:bad_mouse\n");
            return true;
        }

        uint16_t flags = 0;
        uint32_t data = 0;
        switch (button) {
        case 1:
            flags = down ? 0x0002 : 0x0004;
            break;
        case 2:
            flags = down ? 0x0020 : 0x0040;
            break;
        case 3:
            flags = down ? 0x0008 : 0x0010;
            break;
        case 4:
            flags = down ? 0x0080 : 0x0100;
            data = 1;
            break;
        case 5:
            flags = down ? 0x0080 : 0x0100;
            data = 2;
            break;
        }
        if (!updateMouseButtons(flags, data)) {
            QUEUE_ERROR("err:hid_send_failed\n");
            return true;
        }
        renewLease();
        if (EnableKeyCommandAcks) {
            QUEUE_TEXT("ok:mouse_button\n");
        }
        return true;
    }

    if (strncmp(line, "mouse_wheel:", 12) == 0 || strncmp(line, "mouse_hwheel:", 13) == 0) {
        const bool horizontal = line[6] == 'h';
        char *end = nullptr;
        const long amount = strtol(line + (horizontal ? 13 : 12), &end, 10);
        if (end == line + (horizontal ? 13 : 12) || *end != '\0' ||
            amount < -MaxMouseDeltaPerCommand || amount > MaxMouseDeltaPerCommand) {
            QUEUE_ERROR("err:bad_mouse\n");
            return true;
        }
        if (!sendMouseDelta(0, 0, horizontal ? 0 : amount, horizontal ? amount : 0)) {
            QUEUE_ERROR("err:hid_send_failed\n");
            return true;
        }
        renewLease();
        if (EnableKeyCommandAcks) {
            horizontal ? QUEUE_TEXT("ok:mouse_hwheel\n") : QUEUE_TEXT("ok:mouse_wheel\n");
        }
        return true;
    }

    return false;
}

void enterBootloaderNow() {
    // Reboot into the RP2040 ROM bootloader (BOOTSEL / USB mass-storage).
    rp2040.rebootToBootloader();
    while (true) {
    }
}

void scheduleBootloaderReset() {
    bootloader_reset_requested_at = millis();
    bootloader_reset_pending = true;
    led::signalBootloader();
}

void serviceBootloaderReset(unsigned long now) {
    if (bootloader_reset_pending && now - bootloader_reset_requested_at >= BootloaderResetDelayMs &&
        output.idle() && usb_vendor.ready() && txQueued(vendor_tx) == 0) {
        enterBootloaderNow();
    }
}

void handleCommand(char *raw_line) {
    char *line = trimAndLower(raw_line);

    if (*line == '\0') {
        return;
    }

    if (strcmp(line, "ping") == 0) {
        if (protocol_owner == ProtocolOwner::Binary) {
            QUEUE_ERROR("err:busy\n");
            return;
        }
        renewLease();
        QUEUE_TEXT("pong\n");
        return;
    }

    if (strcmp(line, "hello") == 0) {
        queueHello();
        return;
    }

    if (strcmp(line, "status") == 0) {
        queueStatus();
        return;
    }

    if ((protocol_owner == ProtocolOwner::Binary && !(binary_released && output.idle())) ||
        (protocol_owner == ProtocolOwner::None && releasing_protocol == ProtocolOwner::Binary &&
         !output.idle())) {
        QUEUE_ERROR("err:busy\n");
        return;
    }
    protocol_owner = ProtocolOwner::Legacy;
    binary_released = false;
    active_session = 0;

    if (strcmp(line, "enter_bootloader") == 0 || strcmp(line, "bootloader") == 0) {
        keyboard.releaseAll();
        releaseAllMouseButtons();
        QUEUE_TEXT("ok:enter_bootloader\n");
        scheduleBootloaderReset();
        return;
    }

    if (strcmp(line, "r:all") == 0 || strcmp(line, "release:all") == 0 ||
        strcmp(line, "allup") == 0 || strcmp(line, "reset") == 0) {
        handleReleaseAll();
        return;
    }

    if (strncmp(line, "sync:", 5) == 0) {
        handleSnapshotCommand(line + 5);
        return;
    }

    // Short alias: =w,left_shift is the same as sync:w,left_shift.
    if (line[0] == '=') {
        handleSnapshotCommand(line + 1);
        return;
    }

    if (handleMouseCommand(line)) {
        return;
    }

    bool is_down = false;
    char *key_token = nullptr;

    if ((line[0] == 'd' || line[0] == 'u') && line[1] == ':') {
        is_down = line[0] == 'd';
        key_token = line + 2;
    } else if (line[0] == '+' || line[0] == '-') {
        is_down = line[0] == '+';
        key_token = line + 1;
    } else {
        QUEUE_ERROR("err:bad_command\n");
        return;
    }

    key_token = trimAndLower(key_token);

    KeySpec key{};
    if (!parseKeyToken(key_token, key)) {
        QUEUE_ERROR("err:unknown_key\n");
        return;
    }

    const ApplyResult result = is_down ? keyboard.press(key) : keyboard.release(key);

    respondToKeyResult(result, is_down);
}

void finishLine(CommandParser &parser) {
    if (parser.discarding_line) {
        parser.line_length = 0;
        parser.discarding_line = false;
        return;
    }

    parser.line_buffer[parser.line_length] = '\0';
    handleCommand(parser.line_buffer);
    parser.line_length = 0;
}

void appendCommandByte(CommandParser &parser, uint8_t value) {
    if (value == '\r') {
        finishLine(parser);
        parser.swallow_lf = true;
        return;
    }

    if (value == '\n') {
        if (parser.swallow_lf) {
            parser.swallow_lf = false;
            return;
        }

        finishLine(parser);
        return;
    }

    parser.swallow_lf = false;

    if (parser.discarding_line) {
        return;
    }

    if (value < 0x20 || value > 0x7E) {
        parser.line_length = 0;
        parser.discarding_line = true;
        ++invalid_byte_count;
        QUEUE_ERROR("err:invalid_byte\n");
        return;
    }

    if (parser.line_length >= LineBufferSize - 1) {
        parser.line_length = 0;
        parser.discarding_line = true;
        ++line_too_long_count;
        QUEUE_ERROR("err:line_too_long\n");
        return;
    }

    parser.line_buffer[parser.line_length++] = static_cast<char>(value);
}

// Vendor-HID OUT reports arrive asynchronously through this TinyUSB callback.
// Keep it minimal: validate the report id and hand the payload bytes to the RX
// queue. All parsing and keyboard output happen later in core 0's loop().
