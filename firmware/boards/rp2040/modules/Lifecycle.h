// Lease and legacy framing timeouts.
void serviceLineTimeout(CommandParser &parser, unsigned long now) {
    if ((parser.line_length > 0 || parser.discarding_line) &&
        now - parser.last_rx_byte_at >= LineIdleTimeoutMs) {
        resetParser(parser);
        ++line_timeout_count;
        QUEUE_ERROR("err:line_timeout\n");
    }
}

void serviceFailsafe(unsigned long now) {
    if (protocol_owner == ProtocolOwner::Legacy &&
        (keyboard.hasPressedKeys() || mouse_report.buttons != 0) &&
        now - last_lease_renewed_at >= FailsafeReleaseMs) {
        keyboard.releaseAll();
        protocol_owner = ProtocolOwner::None;
        releaseAllMouseButtons();
        ++failsafe_count;
        led::signalFailsafe();
        QUEUE_TEXT("warn:failsafe_release_all\n");
    }
}
