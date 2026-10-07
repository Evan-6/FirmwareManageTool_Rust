void vendorSetReport(uint8_t id, hid_report_type_t type, const uint8_t *data, uint16_t length) {
    if (!id && length) {
        id = *data++;
        --length;
    }
    if (id != 12 || length != 63 ||
        (type != HID_REPORT_TYPE_OUTPUT && type != HID_REPORT_TYPE_INVALID)) {
        ++binary_rx_errors;
        binary::invalid_report.store(true);
        return;
    }
    if (!queue_try_add(&binary::rx, data)) {
        ++binary_rx_errors;
        binary::overflow.store(true);
    }
}
