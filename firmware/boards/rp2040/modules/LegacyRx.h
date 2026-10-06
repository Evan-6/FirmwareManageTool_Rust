// RP2040 internal module; included once by Firmware.cpp.
void vendorSetReport(uint8_t report_id, hid_report_type_t report_type, uint8_t const *buffer,
                     uint16_t bufsize) {
    (void)report_type;

    const uint8_t *data = buffer;
    uint16_t length = bufsize;
    uint8_t id = report_id;

    // OUT-endpoint data arrives with report_id == 0 and the id as the first byte.
    if (id == 0 && length > 0) {
        id = data[0];
        ++data;
        --length;
    }

    if (report_type != HID_REPORT_TYPE_OUTPUT && report_type != HID_REPORT_TYPE_INVALID) {
        rxPush(RxEventType::BadReportId);
        return;
    }
    if (id == 12) {
        if (length != 63) {
            ++binary_rx_errors;
            binary::invalid_report.store(true);
            return;
        }
        binary::Report packet;
        memcpy(packet.bytes, data, 63);
        if (!queue_try_add(&binary::rx, &packet)) {
            ++binary_rx_errors;
            binary::overflow.store(true);
        }
        return;
    }
    if (id != VendorCommandReportId) {
        rxPush(RxEventType::BadReportId);
        return;
    }

    for (uint16_t i = 0; i < length; ++i) {
        const uint8_t value = data[i];
        if (value == 0) {
            break;
        }
        rxPush(RxEventType::Byte, value);
    }
}

// Drain the Vendor-HID RX queue and run commands on core 0.
// Returns true if any byte was consumed.
bool serviceVendorRx(unsigned long now) {
    uint8_t processed = 0;
    RxEvent event{};
    bool did_work = rx_overflow_pending.exchange(false, std::memory_order_acq_rel);

    if (did_work) {
        // Once any byte is lost, no fragment from the affected line is safe to
        // execute. Bound cleanup to one queue depth; discarding_line preserves
        // the poisoned framing across later loops until a newline is observed.
        uint16_t discarded = 0;
        while (discarded < RxQueueDepth && rxPop(event)) {
            if (event.type == RxEventType::BadReportId) {
                ++bad_report_id_count;
            }
            ++discarded;
        }
        resetParser(vendor_parser);
        vendor_parser.discarding_line = true;
        vendor_parser.last_rx_byte_at = now;
        protocolFault(8);
        vendor_parser.discarding_line = true;
        return true;
    }

    while (processed < RxBudgetPerLoop && rxPop(event)) {
        if (event.type == RxEventType::BadReportId) {
            ++bad_report_id_count;
            QUEUE_ERROR("err:bad_report_id\n");
        } else {
            appendCommandByte(vendor_parser, event.value);
        }
        ++processed;
    }

    if (processed != 0) {
        vendor_parser.last_rx_byte_at = now;
        did_work = true;
    }

    return did_work;
}
