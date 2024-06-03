use std::time;
use std::io;
use std::vec;
use std::mem;
use std::cell::RefCell;
use std::rc::Rc;
use mio;

const DEFAULT_EVENTS_CAPACITY: usize = 1024;

struct TimedEvent {
    callback: Option<Box<dyn FnOnce()>>,
    register_instant: time::Instant,
    delta_time: time::Duration,
    done: bool,
}

struct FdEvent {
    callback: Box<dyn FnOnce()>,
}

enum Event {
    Timed(TimedEvent),
    Fd(FdEvent),
    Exit,
}

pub struct SpEvents {
    poll: RefCell<mio::Poll>,
    fd_events: RefCell<vec::Vec<FdEvent>>,
    timed_events: RefCell<vec::Vec<TimedEvent>>,
    exit_events: RefCell<bool>,
}

impl SpEvents {
    pub fn new() -> Result<Self, std::io::Error> {
        let poll = match mio::Poll::new() {
            Ok(poll) => poll, 
            Err(e) => return Err(e)
        };

        Ok(SpEvents {poll: RefCell::new(poll),
                     fd_events: RefCell::new(vec::Vec::new()),
                     timed_events: RefCell::new(vec::Vec::new()),
                     exit_events: RefCell::new(false)
                    })
    }

    pub fn e_queue(&self, func: impl FnOnce() + 'static, delta_time: time::Duration) -> i32 {
        let event = TimedEvent {callback: Some(Box::new(func)), register_instant: time::Instant::now(), delta_time: delta_time, done: false };
        self.timed_events.borrow_mut().push(event);
        0
    }

    pub fn e_exit_events(&self) {
        *self.exit_events.borrow_mut() = true;
    }

    fn get_ready_events(&self) -> vec::Vec<TimedEvent> {
        let mut ready_events = vec::Vec::new();
        let mut i = 0;
        let mut te = self.timed_events.borrow_mut();

        while i < te.len() {
            let event = &te[i];
            if event.register_instant.elapsed() > event.delta_time {
                let e = te.swap_remove(i);
                ready_events.push(e);
            } else {
                i += 1;
            }
        }
        ready_events
    }

    fn get_next_timeout(&self) -> Option<time::Duration> {
        let te = self.timed_events.borrow();
        if te.is_empty() {
            return None
        }

        let event = &te[0];
        let mut min_timeout = event.delta_time.saturating_sub(event.register_instant.elapsed());
        for event in te.iter() {
            let new_timeout = event.delta_time.saturating_sub(event.register_instant.elapsed());
            println!("new timeout {:?}", new_timeout);
            if new_timeout < min_timeout {
                min_timeout = new_timeout;
            }
        }
        Some(min_timeout)
    }

    pub fn e_handle_events(&self) {

        let mut mio_events = mio::Events::with_capacity(DEFAULT_EVENTS_CAPACITY);

        loop {
            // Check whether we should exit
            if *self.exit_events.borrow() {
                return;
            }

            // Handle timed events
            let mut ready_events = self.get_ready_events();
            for event in ready_events.iter_mut() {
                println!("Doing event: register_instant.elapsed {:?}, delta time {:?}", event.register_instant.elapsed(), event.delta_time);
                if let Some(cb) = mem::replace(&mut event.callback, None) {
                    (cb)();
                }
            }

            // Poll to check if we have events waiting for us.
            let timeout = self.get_next_timeout();
            if let Err(err) = self.poll.borrow_mut().poll(&mut mio_events, timeout) {
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                std::process::exit(1);
            }

            // Process all ready events. Note that spurious wakeups are possible, and that we are
            // required to read until we get a WouldBlock error; otherwise, we are not guaranteed to be
            // notified the next time there is data ready to read.
            /*
             for event in events.iter() {
                match event.token() {
                    UDP_SOCKET => loop {
                    } // end UDP recv loop

                    // We only registered one token (UDP_SOCKET), so should never trigger on any other
                    _ => unreachable!(),
                } // end event match
            } // end event iteration
            */

            if mio_events.is_empty() {
                println!("timeout...nothing recceived for {:?}", timeout);
            }
        }
    }
}

pub fn add(left: usize, right: usize) -> usize {
    left + right
}

pub fn say_hello(events: Rc<SpEvents>) {
    println!("hello");
    let ev_clone = events.clone();
    events.e_queue(move || { say_hello(ev_clone); }, time::Duration::from_millis(5000));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }

    #[test]
    fn e_basics() {
        let my_events = Rc::new(SpEvents::new().unwrap());

        let result = my_events.e_queue(|| { add(3, 4); }, time::Duration::from_millis(500));
        assert_eq!(result, 0);

        let ev_clone1 = my_events.clone();
        let result = my_events.e_queue(move || { say_hello(ev_clone1); }, time::Duration::from_millis(5000));
        assert_eq!(result, 0);

        let ev_clone2 = my_events.clone();
        let result = my_events.e_queue(move || { ev_clone2.e_exit_events(); }, time::Duration::from_millis(20000));
        assert_eq!(result, 0);

        my_events.e_handle_events();
    }
}
