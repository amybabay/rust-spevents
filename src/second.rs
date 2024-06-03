use std::time;
use std::io;
use std::vec;
use std::sync::mpsc;
use mio;

const DEFAULT_EVENTS_CAPACITY: usize = 1024;

struct TimedEvent {
    callback: Box<dyn FnMut()>,
    register_instant: time::Instant,
    delta_time: time::Duration,
    done: bool,
}

struct FdEvent {
    callback: Box<dyn FnMut()>,
}

enum Event {
    Timed(TimedEvent),
    Fd(FdEvent),
    Exit,
}

pub struct SpEvents {
    poll: mio::Poll,
    fd_events: mio::event::Events,
    timed_events: vec::Vec<TimedEvent>,
    exit_events: bool,
    event_tx: mpsc::Sender<Event>,
    event_rx: mpsc::Receiver<Event>,
}

#[derive(Clone)]
pub struct SpEventsHandle {
    event_tx: mpsc::Sender<Event>,
}

impl SpEventsHandle {
    //pub fn e_queue(&mut self, func: impl FnOnce() + 'static, delta_time: time::Duration) -> i32 {
    pub fn e_queue(&self, func: impl FnMut() + 'static, delta_time: time::Duration) -> i32 {
        let event = TimedEvent {callback: Box::new(func), register_instant: time::Instant::now(), delta_time: delta_time, done: false };
        self.event_tx.send(Event::Timed(event)).unwrap();
        0
    }

    pub fn e_exit_events(&self) -> i32 {
        self.event_tx.send(Event::Exit).unwrap();
        0
    }
}

impl SpEvents {
    pub fn new() -> Result<Self, std::io::Error> {
        let poll = match mio::Poll::new() {
            Ok(poll) => poll, 
            Err(e) => return Err(e)
        };
        let fd_events = mio::Events::with_capacity(DEFAULT_EVENTS_CAPACITY);
        let (tx, rx) = mpsc::channel();

        Ok(SpEvents {poll: poll, fd_events: fd_events, timed_events: vec::Vec::new(), exit_events: false, event_tx: tx, event_rx: rx })
    }

    pub fn get_handle(&self) -> SpEventsHandle {
        SpEventsHandle {event_tx: self.event_tx.clone()}
    }

    fn e_queue_internal(&mut self, event: TimedEvent) {
        self.timed_events.push(event);
    }

    pub fn e_handle_events(&mut self) {

        loop {
            // Check whether we should exit
            if self.exit_events {
                return;
            }

            loop {
                let new_event = match self.event_rx.try_recv() {
                    Ok(event) => event,
                    Err(_e) => break
                };

                match new_event {
                    Event::Timed(event) => self.e_queue_internal(event),
                    Event::Fd(_) => todo!(),
                    Event::Exit => self.exit_events = true,
                }
            }

            // Handle timed events
            for event in self.timed_events.iter_mut() {
                if !event.done && event.register_instant.elapsed() > event.delta_time {
                    println!("Doing event: register_instant.elapsed {:?}, delta time {:?}", event.register_instant.elapsed(), event.delta_time);
                    (event.callback)(); // call the callback function
                    event.done = true;
                }
            }

            // Poll to check if we have events waiting for us.
            if let Err(err) = self.poll.poll(&mut self.fd_events, Some(time::Duration::new(10, 0))) { // 10 second timeout
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

            if self.fd_events.is_empty() {
                println!("timeout...nothing recceived for 10 seconds.");
            }
        }
    }
}

pub fn add(left: usize, right: usize) -> usize {
    left + right
}

pub fn say_hello(events: SpEventsHandle) {
    println!("hello");
    let handle = events.clone();
    events.e_queue(move || { say_hello(handle.clone()); }, time::Duration::from_millis(5000));
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
        let mut my_events = SpEvents::new().unwrap();
        let ev_handle = my_events.get_handle();

        let result = ev_handle.e_queue(|| { add(3, 4); }, time::Duration::from_millis(500));
        assert_eq!(result, 0);

        let ev_handle2 = my_events.get_handle();
        let result = ev_handle.e_queue(move || { ev_handle2.e_exit_events(); }, time::Duration::from_millis(30000));
        assert_eq!(result, 0);

        let ev_handle3 = my_events.get_handle();
        let result = ev_handle.e_queue(move || { say_hello(ev_handle3.clone()); }, time::Duration::from_millis(5000));
        assert_eq!(result, 0);

        my_events.e_handle_events();
    }
}
