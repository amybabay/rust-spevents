use std::time;
use std::io;
use std::vec;
use std::net::{ToSocketAddrs, Ipv4Addr};
use mio;

const DEFAULT_EVENTS_CAPACITY: usize = 1024;
const MAX_MSG_SIZE: usize = 1024;

struct TimedEvent {
    callback: fn(&mut SpEvents),
    register_instant: time::Instant,
    delta_time: time::Duration,
}

// Use AnyFdEvent trait so that we can have a collection of FdEvents that may operate on different
// mio:event:Source types
trait AnyFdEvent {
    fn do_callback(&self, events: &mut SpEvents);
    fn get_token(&self) -> mio::Token;
    fn get_source(&mut self) -> &mut dyn mio::event::Source;
}

struct FdEvent<S>
where S: mio::event::Source + ?Sized,
{
    callback: fn(&S, &mut SpEvents),
    source: Box<S>,
    token: mio::Token,
}

impl <S: mio::event::Source + ?Sized> AnyFdEvent for FdEvent<S> {
    fn do_callback(&self, events: &mut SpEvents) {
        (self.callback)(&self.source, events);
    }

    fn get_token(&self) -> mio::Token {
        self.token
    }

    fn get_source(&mut self) -> &mut dyn mio::event::Source {
        &mut self.source
    }
}

pub enum Priority {
    LowPriority,
    MediumPriority,
    HighPriority,
}

pub struct SpEvents {
    poll: mio::Poll,
    fd_events: vec::Vec<Box<dyn AnyFdEvent>>,
    timed_events: vec::Vec<TimedEvent>,
    exit_events: bool, // true if we should exit on next loop; false otherwise
    next_token: usize, // next token to use when registering an FD event
}

impl SpEvents {
    pub fn new() -> Result<Self, std::io::Error> {
        // Create mio poll
        let poll = match mio::Poll::new() {
            Ok(poll) => poll, 
            Err(e) => return Err(e)
        };

        // Build SpEvents struct with empty data structures
        Ok(SpEvents {poll: poll,
                     fd_events: vec::Vec::new(),
                     timed_events: vec::Vec::new(),
                     exit_events: false,
                     next_token: 0
                    })
    }

    /// Schedules callback function `func` to be called after time `delta_time` elapses
    pub fn e_queue(&mut self, func: fn(&mut SpEvents), delta_time: time::Duration) -> i32 {
        println!("Queueing event: delta time {:?}", delta_time);
        let event = TimedEvent {callback: func, 
                                register_instant: time::Instant::now(),
                                delta_time: delta_time};
        self.timed_events.push(event);
        0
    }

    /// Un-schedules callback function `func`
    pub fn e_dequeue(&mut self, func: fn(&mut SpEvents)) -> i32 {
        let mut i = 0;
        let te = &mut self.timed_events;

        while i < te.len() {
            let event = &te[i];
            if event.callback == func {
                te.swap_remove(i);
                println!("Dequeued event: {:?}", func);
                return 0 // should we enforce that there is only one registered instance of a particular function?
            } else {
                i += 1;
            }
        }
        println!("e_dequeue event not found: {:?}", func);
        -1
    }

    /// Attaches callback function `func` to mio event source `source`, such that `func` is called
    /// each time mio interest `interest` (e.g. readable, writeable) is ready
    pub fn e_attach_fd<S>(&mut self, mut source: S, interest: mio::Interest, func: fn(&S, &mut SpEvents), priority: Priority) -> io::Result<()>
    where S: mio::event::Source + 'static,
    {
        let event_count = self.fd_events.len();
        if event_count == DEFAULT_EVENTS_CAPACITY {
            return Err(io::Error::new(io::ErrorKind::Other, "Maximum number of FD events already registered"))
        }

        let token = mio::Token(self.next_token);
        if let Err(err) = self.poll.registry().register(&mut source, token, interest) {
            return Err(err)
        }
        self.next_token += 1;

        let event = FdEvent {callback: func,
                             source: Box::new(source),
                             token: token};

        self.fd_events.push(Box::new(event));
        Ok(())
    }

    /* Should this be based on token (which we don't currently return from e_attach) or on the
     * source itself? */
    /// Detaches callback previously registered with mio Token `token`
    pub fn e_detach_fd(&mut self, token: mio::Token) -> i32
    {
        if let Some(mut ev) = self.get_fd_event_by_token(token) {
            self.poll.registry().deregister(ev.get_source());
            return 0
        }
        return -1
    }

    /// Set `self.exit_events` to true, so that we will exit the event loop on the next iteration
    pub fn e_exit_events(&mut self) {
        self.exit_events = true;
    }

    fn get_ready_events(&mut self) -> vec::Vec<TimedEvent> {
        let mut ready_events = vec::Vec::new();
        let mut i = 0;
        let te = &mut self.timed_events;

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
        let te = &self.timed_events;
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

    //fn get_fd_event_by_token(&self, token: mio::Token) -> Option<&dyn AnyFdEvent> {
    fn get_fd_event_by_token(&mut self, token: mio::Token) -> Option<Box<dyn AnyFdEvent>> {
        /*
        for event in self.fd_events.iter() {
            if event.get_token() == token {
                return Some(&**event)
            }
        }
        None
        */

        let mut i = 0;
        let fe = &mut self.fd_events;

        while i < fe.len() {
            let event = &fe[i];
            if event.get_token() == token {
                let e = fe.swap_remove(i);
                return Some(e)
            } else {
                i += 1;
            }
        }
        None
    }

    /// Start the event loop. Normally this is called after scheduling some timed events and/or
    /// attaching some fd events. This will run until `e_exit_events` is called (if no scheduled /
    /// attached event ever calls `e_exit_events`, the loop will run forever)
    pub fn e_handle_events(&mut self) {
        let mut mio_events = mio::Events::with_capacity(DEFAULT_EVENTS_CAPACITY);
        self.exit_events = false; // enables calling e_handle_events again after exiting the event loop

        loop {
            // Handle timed events
            let mut ready_events = self.get_ready_events();
            for event in ready_events.iter_mut() {
                println!("Doing event: register_instant.elapsed {:?}, delta time {:?}", event.register_instant.elapsed(), event.delta_time);
                (event.callback)(self);

                // Check whether we should exit
                if self.exit_events {
                    return;
                }
            }

            // Poll to check if we have events waiting for us.
            let timeout = self.get_next_timeout();
            if let Err(err) = self.poll.poll(&mut mio_events, timeout) {
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                std::process::exit(1);
            }

            // Process all ready fd events. Note that spurious wakeups are possible, and that we are
            // required to read until we get a WouldBlock error; otherwise, we are not guaranteed to be
            // notified the next time there is data ready to read.
            for event in mio_events.iter() {
                // Hack! get_fd_event_by_token removes the event from our data structures. Then, we
                // do the callback and then re-add it. If we do not remove the event before calling
                // the callback, this does not compile, because both "ev" and "self" are considered
                // references to "self" in "ev.do_callback(self)"
                let event_data = self.get_fd_event_by_token(event.token());
                if let Some(ev) = event_data {
                    ev.do_callback(self);
                    self.fd_events.push(ev);
                }

                // Check whether we should exit
                if self.exit_events {
                    return;
                }
            } // end event iteration

            if mio_events.is_empty() {
                println!("timeout...nothing recceived for {:?}", timeout);
            }
        }
    }
}

pub fn add(left: usize, right: usize) -> usize {
    println!("----- adding {} + {}", left, right);
    left + right
}

pub fn say_hello(events: &mut SpEvents) {
    println!("hello");
    events.e_queue(say_hello, time::Duration::from_millis(5000));
}

pub fn exit_events(events: &mut SpEvents) {
    events.e_exit_events();
}


pub fn send_msg(msg: &str, addr: &str) {
    let socket = mio::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0).into()).unwrap();
    let dest_addr = addr.to_socket_addrs().unwrap().next().unwrap();
    let bytes = socket.send_to(msg.as_bytes(), dest_addr).unwrap();
    println!("Sent {bytes:?}  bytes: {}", msg);
}

pub fn receive_msg(socket: &mio::net::UdpSocket, _: &mut SpEvents) {
    let mut buf: [u8; MAX_MSG_SIZE] = [0; MAX_MSG_SIZE];
    match socket.recv_from(&mut buf) {
        Ok((bytes, from_addr)) => {
            println!("Received {bytes:?} bytes from {}: {}",
                     from_addr,
                     std::str::from_utf8(&buf[0..bytes]).unwrap());
        }
        Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
            println!("Nothing to read! Would block");
        }
        Err(err) => {
            println!("Error receiving! {}", err);
        }
    }
}

pub fn receive_msg2(socket: &mio::net::UdpSocket, events: &mut SpEvents) {
    let mut buf: [u8; MAX_MSG_SIZE] = [0; MAX_MSG_SIZE];
    match socket.recv_from(&mut buf) {
        Ok((bytes, from_addr)) => {
            println!("Received {bytes:?} bytes from {}: {}",
                     from_addr,
                     std::str::from_utf8(&buf[0..bytes]).unwrap());
        }
        Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
            println!("Nothing to read! Would block");
        }
        Err(err) => {
            println!("Error receiving! {}", err);
        }
    }

    events.e_queue(|_| { add(7, 8);}, time::Duration::from_secs(1));

    // will panic if receive_msg2 is called more than once
    let socket =  mio::net::UdpSocket::bind("127.0.0.1:8888".parse().unwrap()).unwrap();
    events.e_attach_fd(socket, mio::Interest::READABLE, receive_msg, Priority::HighPriority).unwrap();
    events.e_queue(|_| { send_msg("***** this is a test message", "127.0.0.1:8888"); }, time::Duration::from_secs(2));
    events.e_queue(|_| { send_msg("***** this is another test message", "127.0.0.1:8888"); }, time::Duration::from_secs(7));
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
    fn e_timed() {
        let mut my_events = SpEvents::new().unwrap();

        let result = my_events.e_queue(|_| { add(3, 4); }, time::Duration::from_millis(500));
        assert_eq!(result, 0);

        let result = my_events.e_queue(say_hello, time::Duration::from_secs(5));
        assert_eq!(result, 0);

        let result = my_events.e_queue(say_hello, time::Duration::from_secs(3));
        assert_eq!(result, 0);

        let result = my_events.e_queue(exit_events, time::Duration::from_secs(20));
        assert_eq!(result, 0);

        let result = my_events.e_dequeue(say_hello);
        assert_eq!(result, 0);

        /*
        loop {
            let result = my_events.e_dequeue(say_hello);
            if result < 0 {
                break;
            }
        }
        */

        my_events.e_handle_events();

        println!("\nReturned from e_handle_events!!");

        let result = my_events.e_queue(|_| { add(3333, 4444); }, time::Duration::from_millis(500));
        assert_eq!(result, 0);

        let result = my_events.e_queue(exit_events, time::Duration::from_secs(2));
        assert_eq!(result, 0);

        my_events.e_handle_events();
    }

    #[test]
    fn e_fd_minimal() {
        let mut my_events = SpEvents::new().unwrap();
        let socket =  mio::net::UdpSocket::bind("127.0.0.1:5555".parse().unwrap()).unwrap();

        my_events.e_attach_fd(socket, mio::Interest::READABLE, |_,_| { add(2, 5); }, Priority::HighPriority).unwrap();
        my_events.e_queue(|_| { send_msg("----- this is a test message", "127.0.0.1:5555"); }, time::Duration::from_secs(2));
        my_events.e_queue(|_| { send_msg("----- this is another test message", "127.0.0.1:5555"); }, time::Duration::from_secs(7));

        let result = my_events.e_queue(exit_events, time::Duration::from_secs(21));
        assert_eq!(result, 0);


        my_events.e_handle_events();
    }

    #[test]
    fn e_fd() {
        let mut my_events = SpEvents::new().unwrap();

        let socket =  mio::net::UdpSocket::bind("127.0.0.1:6666".parse().unwrap()).unwrap();
        my_events.e_attach_fd(socket, mio::Interest::READABLE, receive_msg, Priority::HighPriority).unwrap();
        my_events.e_queue(|_| { send_msg("++++++ this is a test message", "127.0.0.1:6666"); }, time::Duration::from_secs(2));
        my_events.e_queue(|_| { send_msg("++++++ this is another test message", "127.0.0.1:6666"); }, time::Duration::from_secs(7));

        let socket2 =  mio::net::UdpSocket::bind("127.0.0.1:7777".parse().unwrap()).unwrap();
        my_events.e_attach_fd(socket2, mio::Interest::READABLE, receive_msg2, Priority::HighPriority).unwrap();
        my_events.e_queue(|_| { send_msg("====== this is a test message", "127.0.0.1:7777"); }, time::Duration::from_secs(2));
        //my_events.e_queue(|_| { send_msg("====== this is another test message", "127.0.0.1:7777"); }, time::Duration::from_secs(7));

        let result = my_events.e_queue(exit_events, time::Duration::from_secs(21));
        assert_eq!(result, 0);


        my_events.e_handle_events();
    }
}
