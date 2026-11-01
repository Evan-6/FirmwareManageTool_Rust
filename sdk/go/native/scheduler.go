package gdbnative

import (
	"container/heap"
	"sync"
	"sync/atomic"
	"time"
)

// TaskID identifies a scheduled job.
type TaskID uint64

// TaskError is one failed job, shaped like gdb_scheduler_take_errors' JSON.
type TaskError struct {
	ID      TaskID `json:"id"`
	Code    Code   `json:"code"`
	Message string `json:"message"`
}

// maxErrors is how many failures the scheduler keeps before dropping the oldest.
const maxErrors = 32

type jobAction struct {
	once   func(*Device) error
	repeat func(*Device) error
}

type job struct {
	id       TaskID
	dueMS    uint64
	priority int32
	seq      uint64
	everyMS  uint64
	action   jobAction
	// index is maintained by container/heap.
	index int
}

// jobQueue orders jobs by due time, then priority (higher first), then
// submission order — the same ordering sdk/rust/src/task.rs documents.
type jobQueue []*job

func (q jobQueue) Len() int { return len(q) }

func (q jobQueue) Less(i, j int) bool {
	if q[i].dueMS != q[j].dueMS {
		return q[i].dueMS < q[j].dueMS
	}
	if q[i].priority != q[j].priority {
		return q[i].priority > q[j].priority
	}
	return q[i].seq < q[j].seq
}

func (q jobQueue) Swap(i, j int) {
	q[i], q[j] = q[j], q[i]
	q[i].index = i
	q[j].index = j
}

func (q *jobQueue) Push(value any) {
	entry := value.(*job)
	entry.index = len(*q)
	*q = append(*q, entry)
}

func (q *jobQueue) Pop() any {
	old := *q
	last := len(old) - 1
	entry := old[last]
	old[last] = nil
	*q = old[:last]
	return entry
}

type schedulerMessage struct {
	kind  int
	job   *job
	id    TaskID
	pause bool
	reply chan struct{}
}

const (
	messageSubmit = iota
	messageCancel
	messagePause
	messageWait
	messageShutdown
)

// Scheduler runs jobs on one goroutine owned by the device it was created
// with: one at a time, in (due time, priority, submission order) order.
type Scheduler struct {
	mu       sync.Mutex
	queue    jobQueue
	messages chan schedulerMessage
	paused   atomic.Bool
	started  time.Time

	nextID  atomic.Uint64
	nextSeq atomic.Uint64

	errorMu sync.Mutex
	errors  []TaskError

	cancelMu  sync.Mutex
	cancelled map[TaskID]struct{}

	done  chan struct{}
	stop  sync.Once
	state atomic.Bool
}

// Scheduler creates a scheduler bound to this device.
func (d *Device) Scheduler() *Scheduler {
	scheduler := &Scheduler{
		messages:  make(chan schedulerMessage, 64),
		started:   time.Now(),
		cancelled: make(map[TaskID]struct{}),
		done:      make(chan struct{}),
	}
	scheduler.nextID.Store(1)
	scheduler.nextSeq.Store(1)
	go scheduler.worker(d)
	return scheduler
}

// Submit runs a callback once after delay.
func (s *Scheduler) Submit(delay time.Duration, priority int32, action func(*Device) error) (TaskID, error) {
	if action == nil {
		return 0, errorf(Invalid, "a task needs a callback")
	}
	return s.enqueue(delay, priority, 0, jobAction{once: action})
}

// Every runs a callback repeatedly until cancelled; the first run is one
// interval after submission.
func (s *Scheduler) Every(interval time.Duration, priority int32, action func(*Device) error) (TaskID, error) {
	if interval <= 0 {
		return 0, errorf(Invalid, "interval must be positive")
	}
	if action == nil {
		return 0, errorf(Invalid, "a task needs a callback")
	}
	return s.enqueue(interval, priority, uint64(interval/time.Millisecond), jobAction{repeat: action})
}

func (s *Scheduler) enqueue(delay time.Duration, priority int32, everyMS uint64, action jobAction) (TaskID, error) {
	if s.state.Load() {
		return 0, errorf(IO, "scheduler is shut down")
	}
	id := TaskID(s.nextID.Add(1) - 1)
	entry := &job{
		id:       id,
		dueMS:    uint64(time.Since(s.started)/time.Millisecond) + uint64(delay/time.Millisecond),
		priority: priority,
		seq:      s.nextSeq.Add(1) - 1,
		everyMS:  everyMS,
		action:   action,
	}
	select {
	case s.messages <- schedulerMessage{kind: messageSubmit, job: entry}:
		return id, nil
	default:
		return 0, errorf(IO, "scheduler is not accepting jobs")
	}
}

// Cancel removes a queued or repeating task. A cancel that races a due job
// still wins as long as the callback has not started.
func (s *Scheduler) Cancel(id TaskID) {
	s.cancelMu.Lock()
	s.cancelled[id] = struct{}{}
	s.cancelMu.Unlock()
	if s.state.Load() {
		return
	}
	select {
	case s.messages <- schedulerMessage{kind: messageCancel, id: id}:
	default:
	}
}

// Pause stops executing queued jobs.
func (s *Scheduler) Pause() {
	s.paused.Store(true)
}

// Resume continues executing queued jobs.
func (s *Scheduler) Resume() { s.paused.Store(false) }

// WaitIdle blocks until the queue is empty and no job is running.
func (s *Scheduler) WaitIdle(timeout time.Duration) error {
	reply := make(chan struct{})
	select {
	case s.messages <- schedulerMessage{kind: messageWait, reply: reply}:
	case <-time.After(timeout):
		return errorf(Timeout, "scheduler did not become idle")
	}
	select {
	case <-reply:
		return nil
	case <-time.After(timeout):
		return errorf(Timeout, "scheduler did not become idle")
	}
}

// TakeErrors returns the failures seen so far and clears them.
func (s *Scheduler) TakeErrors() []TaskError {
	s.errorMu.Lock()
	defer s.errorMu.Unlock()
	out := s.errors
	s.errors = nil
	return out
}

// TakeErrorsJSON returns the failures as JSON, the shape
// gdb_scheduler_take_errors produces.
func (s *Scheduler) TakeErrorsJSON() (string, error) {
	errors := s.TakeErrors()
	if errors == nil {
		errors = []TaskError{}
	}
	return jsonString(errors)
}

// isCancelled consumes a pending cancellation.
func (s *Scheduler) isCancelled(id TaskID) bool {
	s.cancelMu.Lock()
	defer s.cancelMu.Unlock()
	if _, ok := s.cancelled[id]; ok {
		delete(s.cancelled, id)
		return true
	}
	return false
}

// Close shuts the scheduler down and waits for the worker to unwind. It is
// safe to call twice and safe to call from a task.
func (s *Scheduler) Close() {
	s.stop.Do(func() {
		s.state.Store(true)
		select {
		case s.messages <- schedulerMessage{kind: messageShutdown}:
		case <-time.After(time.Second):
		}
		<-s.done
	})
}

func (s *Scheduler) worker(device *Device) {
	defer close(s.done)
	waiters := make([]chan struct{}, 0, 4)
	for {
		paused := s.paused.Load()
		wait := 20 * time.Millisecond
		if !paused {
			s.mu.Lock()
			if len(s.queue) > 0 {
				remaining := time.Duration(s.queue[0].dueMS)*time.Millisecond - time.Since(s.started)
				switch {
				case remaining < time.Millisecond:
					wait = time.Millisecond
				case remaining < 20*time.Millisecond:
					wait = remaining
				}
			}
			s.mu.Unlock()
		}
		select {
		case message := <-s.messages:
			if shutdown := s.handle(message, &waiters); shutdown {
				for _, waiter := range waiters {
					close(waiter)
				}
				return
			}
		case <-time.After(wait):
		}
		// Drain whatever is already queued: a burst of submits must all be in
		// the heap before anything runs, or (due, priority, seq) would depend on
		// how fast the caller happened to hand the jobs over.
	drain:
		for {
			select {
			case message := <-s.messages:
				if shutdown := s.handle(message, &waiters); shutdown {
					for _, waiter := range waiters {
						close(waiter)
					}
					return
				}
			default:
				break drain
			}
		}
		if !s.paused.Load() {
			s.runDue(device)
		}
		s.mu.Lock()
		empty := len(s.queue) == 0
		s.mu.Unlock()
		if empty {
			for _, waiter := range waiters {
				close(waiter)
			}
			waiters = waiters[:0]
		}
	}
}

// handle applies one message; it reports whether the worker must stop.
func (s *Scheduler) handle(message schedulerMessage, waiters *[]chan struct{}) bool {
	switch message.kind {
	case messageSubmit:
		s.mu.Lock()
		heap.Push(&s.queue, message.job)
		s.mu.Unlock()
	case messageCancel:
		s.cancelMu.Lock()
		s.cancelled[message.id] = struct{}{}
		s.cancelMu.Unlock()
	case messagePause:
		s.paused.Store(message.pause)
	case messageWait:
		*waiters = append(*waiters, message.reply)
	case messageShutdown:
		return true
	}
	return false
}

// runDue executes every job whose due time has passed.
func (s *Scheduler) runDue(device *Device) {
	for {
		now := uint64(time.Since(s.started) / time.Millisecond)
		s.mu.Lock()
		if len(s.queue) == 0 || s.queue[0].dueMS > now {
			s.mu.Unlock()
			return
		}
		entry := heap.Pop(&s.queue).(*job)
		s.mu.Unlock()

		if s.isCancelled(entry.id) {
			continue
		}
		var err error
		if entry.action.once != nil {
			action := entry.action.once
			entry.action.once = nil
			err = action(device)
		} else if entry.action.repeat != nil {
			err = entry.action.repeat(device)
		}
		if err != nil {
			s.record(entry.id, err)
		}
		if entry.everyMS > 0 {
			if s.isCancelled(entry.id) {
				continue
			}
			next := entry.dueMS + entry.everyMS
			for next <= now {
				next += entry.everyMS
			}
			entry.dueMS = next
			s.mu.Lock()
			heap.Push(&s.queue, entry)
			s.mu.Unlock()
		}
	}
}

func (s *Scheduler) record(id TaskID, err error) {
	entry := TaskError{ID: id, Code: CodeOf(err)}
	if failure, ok := err.(*Error); ok {
		entry.Message = failure.Message
	}
	s.errorMu.Lock()
	defer s.errorMu.Unlock()
	if len(s.errors) >= maxErrors {
		s.errors = s.errors[1:]
	}
	s.errors = append(s.errors, entry)
}
