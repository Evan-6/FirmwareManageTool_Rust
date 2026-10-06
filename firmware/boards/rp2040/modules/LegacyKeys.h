// RP2040 internal module; included once by Firmware.cpp.
constexpr uint32_t fnv1aConst(const char *text, uint32_t hash = 2166136261UL) {
    return *text == '\0' ? hash
                         : fnv1aConst(text + 1, (hash ^ static_cast<uint8_t>(*text)) * 16777619UL);
}

uint32_t fnv1a(const char *text) {
    uint32_t hash = 2166136261UL;

    while (*text != '\0') {
        hash ^= static_cast<uint8_t>(*text++);
        hash *= 16777619UL;
    }

    return hash;
}

bool parseFunctionKey(const char *token, KeySpec &result) {
    if (token[0] != 'f' || token[1] < '1' || token[1] > '9') {
        return false;
    }

    uint16_t number = 0;
    const char *cursor = token + 1;

    while (*cursor >= '0' && *cursor <= '9') {
        number = static_cast<uint16_t>(number * 10 + (*cursor - '0'));
        if (number > 24) {
            return false;
        }
        ++cursor;
    }

    if (*cursor != '\0' || number < 1 || number > 24) {
        return false;
    }

    const uint8_t usage = number <= 12 ? static_cast<uint8_t>(0x3A + number - 1)
                                       : static_cast<uint8_t>(0x68 + number - 13);

    result = usageKey(usage);
    return true;
}

bool parseKeyToken(const char *token, KeySpec &result) {
    if (token[0] == '\0') {
        return false;
    }

    if (token[1] == '\0') {
        const char key = token[0];

        if (key >= 'a' && key <= 'z') {
            result = usageKey(static_cast<uint8_t>(0x04 + key - 'a'));
            return true;
        }

        if (key >= '1' && key <= '9') {
            result = usageKey(static_cast<uint8_t>(0x1E + key - '1'));
            return true;
        }

        if (key == '0') {
            result = usageKey(0x27);
            return true;
        }

        return false;
    }

    if (parseFunctionKey(token, result)) {
        return true;
    }

    const uint32_t hash = fnv1a(token);

#define KEY_CASE(name, value)                                                                      \
    case fnv1aConst(name):                                                                         \
        if (strcmp(token, name) == 0) {                                                            \
            result = value;                                                                        \
            return true;                                                                           \
        }                                                                                          \
        break

    switch (hash) {
        KEY_CASE("left", usageKey(0x50));
        KEY_CASE("right", usageKey(0x4F));
        KEY_CASE("up", usageKey(0x52));
        KEY_CASE("down", usageKey(0x51));

        KEY_CASE("alt", modifierKey(ModLeftAlt));
        KEY_CASE("left_alt", modifierKey(ModLeftAlt));
        KEY_CASE("right_alt", modifierKey(ModRightAlt));

        KEY_CASE("ctrl", modifierKey(ModLeftCtrl));
        KEY_CASE("control", modifierKey(ModLeftCtrl));
        KEY_CASE("left_ctrl", modifierKey(ModLeftCtrl));
        KEY_CASE("right_ctrl", modifierKey(ModRightCtrl));

        KEY_CASE("shift", modifierKey(ModLeftShift));
        KEY_CASE("left_shift", modifierKey(ModLeftShift));
        KEY_CASE("right_shift", modifierKey(ModRightShift));

        KEY_CASE("win", modifierKey(ModLeftGui));
        KEY_CASE("gui", modifierKey(ModLeftGui));
        KEY_CASE("left_gui", modifierKey(ModLeftGui));
        KEY_CASE("right_gui", modifierKey(ModRightGui));

        KEY_CASE("enter", usageKey(0x28));
        KEY_CASE("return", usageKey(0x28));
        KEY_CASE("esc", usageKey(0x29));
        KEY_CASE("escape", usageKey(0x29));
        KEY_CASE("backspace", usageKey(0x2A));
        KEY_CASE("tab", usageKey(0x2B));
        KEY_CASE("space", usageKey(0x2C));
        KEY_CASE("caps_lock", usageKey(0x39));
        KEY_CASE("capslock", usageKey(0x39));

        KEY_CASE("print_screen", usageKey(0x46));
        KEY_CASE("printscreen", usageKey(0x46));
        KEY_CASE("scroll_lock", usageKey(0x47));
        KEY_CASE("scrolllock", usageKey(0x47));
        KEY_CASE("pause", usageKey(0x48));
        KEY_CASE("insert", usageKey(0x49));
        KEY_CASE("home", usageKey(0x4A));
        KEY_CASE("pageup", usageKey(0x4B));
        KEY_CASE("page_up", usageKey(0x4B));
        KEY_CASE("delete", usageKey(0x4C));
        KEY_CASE("end", usageKey(0x4D));
        KEY_CASE("pagedown", usageKey(0x4E));
        KEY_CASE("page_down", usageKey(0x4E));
        KEY_CASE("menu", usageKey(0x65));
        KEY_CASE("application", usageKey(0x65));

        KEY_CASE("minus", usageKey(0x2D));
        KEY_CASE("equal", usageKey(0x2E));
        KEY_CASE("left_bracket", usageKey(0x2F));
        KEY_CASE("right_bracket", usageKey(0x30));
        KEY_CASE("backslash", usageKey(0x31));
        KEY_CASE("semicolon", usageKey(0x33));
        KEY_CASE("quote", usageKey(0x34));
        KEY_CASE("apostrophe", usageKey(0x34));
        KEY_CASE("grave", usageKey(0x35));
        KEY_CASE("backtick", usageKey(0x35));
        KEY_CASE("comma", usageKey(0x36));
        KEY_CASE("period", usageKey(0x37));
        KEY_CASE("dot", usageKey(0x37));
        KEY_CASE("slash", usageKey(0x38));

        KEY_CASE("num_lock", usageKey(0x53));
        KEY_CASE("numlock", usageKey(0x53));
        KEY_CASE("kp_slash", usageKey(0x54));
        KEY_CASE("kp_asterisk", usageKey(0x55));
        KEY_CASE("kp_minus", usageKey(0x56));
        KEY_CASE("kp_plus", usageKey(0x57));
        KEY_CASE("kp_enter", usageKey(0x58));
        KEY_CASE("kp_1", usageKey(0x59));
        KEY_CASE("kp_2", usageKey(0x5A));
        KEY_CASE("kp_3", usageKey(0x5B));
        KEY_CASE("kp_4", usageKey(0x5C));
        KEY_CASE("kp_5", usageKey(0x5D));
        KEY_CASE("kp_6", usageKey(0x5E));
        KEY_CASE("kp_7", usageKey(0x5F));
        KEY_CASE("kp_8", usageKey(0x60));
        KEY_CASE("kp_9", usageKey(0x61));
        KEY_CASE("kp_0", usageKey(0x62));
        KEY_CASE("kp_dot", usageKey(0x63));
    }

#undef KEY_CASE

    return false;
}
