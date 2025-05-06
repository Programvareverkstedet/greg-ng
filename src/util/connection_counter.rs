use std::fmt;

#[derive(Debug, Clone, Copy)]
pub enum ConnectionEvent {
    Connected,
    Disconnected,
}

impl ConnectionEvent {
    pub fn to_i8(self) -> i8 {
        match self {
            ConnectionEvent::Connected => 1,
            ConnectionEvent::Disconnected => -1,
        }
    }
}

impl fmt::Display for ConnectionEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectionEvent::Connected => write!(f, "Connected"),
            ConnectionEvent::Disconnected => write!(f, "Disconnected"),
        }
    }
}
