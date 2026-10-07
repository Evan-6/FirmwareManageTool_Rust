#pragma once
#include "OutputScheduler.h"
#include "SessionTable.h"
namespace hidfw {
struct Counters {
    uint16_t rx = 0, tx = 0, hid = 0, failsafe = 0;
};
class InputEngine {
  public:
    InputEngine() = default;
    InputEngine(const InputEngine &) = delete;
    InputEngine &operator=(const InputEngine &) = delete;
    enum Notice : uint8_t { Error = 1, Failsafe = 2, BootConfirmed = 4 };
    void handle(const wire::Report &report, uint32_t now);
    void receiveFault(uint8_t code, uint16_t errors, uint32_t now);
    bool advance(uint32_t now, bool mounted, uint8_t completed, bool failed, bool ready,
                 InputReport &report);
    void inputRejected(uint32_t now);
    void serviceControl(uint32_t now);
    const wire::Report *vendorReply() const;
    void vendorSubmitted();
    void vendorCompleted(uint32_t generation);
    uint32_t bootGeneration() const { return boot_generation_; }
    uint32_t rxGeneration() const { return rx_generation_; }
    uint8_t takeNotices() {
        auto n = notices_;
        notices_ = 0;
        return n;
    }
    uint8_t sessionCount() const { return sessions_.count(); }
    const Session *session(uint32_t id) const { return sessions_.find(id); }
    const InputState &desiredState() const { return output_.desired(); }
    const InputState &completedState() const { return output_.completed(); }
    const Counters &counters() const { return counters_; }
    bool idle() const { return output_.idle(); }
    bool txEmpty() const { return tx_count_ == 0; }
    bool bootRequested() const { return boot_requested_; }
    bool bootConfirmed() const { return boot_confirmed_; }

  private:
    SessionTable sessions_{};
    OutputScheduler output_{};
    Counters counters_{};
    wire::Report tx_[config::TxDepth]{};
    uint8_t tx_head_ = 0, tx_count_ = 0, notices_ = 0;
    uint32_t now_ = 0, boot_generation_ = 0, rx_generation_ = 0;
    bool mounted_ = false, overflow_ = false;
    bool boot_requested_ = false, boot_inflight_ = false, boot_completed_ = false,
         boot_confirmed_ = false;
    void reply(uint8_t op, uint32_t session, uint32_t seq, const uint8_t *data, uint8_t n);
    void result(uint8_t op, uint32_t session, uint32_t seq, uint8_t code = 0);
    void event(uint8_t code, uint32_t session, uint32_t seq);
    void status(const Session &s, uint32_t seq);
    void sessionFault(Session &s, uint8_t code);
    void fault(uint8_t code);
    void clearBoot();
    void discardReplies(uint32_t session);
    uint16_t leaseRemaining(const Session &s) const;
    bool queueState(uint32_t seq, bool force, uint32_t session);
    bool queueMotion(int32_t x, int32_t y, int32_t wheel, int32_t hwheel, uint32_t seq,
                     uint32_t session);
    bool queueBarrier(uint32_t seq, uint32_t session);
    bool cancelOwner(uint32_t session, uint32_t seq, uint8_t slot);
    bool checkScheduled(bool ok);
};
} // namespace hidfw
