use crate::Score;
pub type Behaviour = (Score, &'static str);

macro_rules! define_behaviour {
    ( $( $name:ident => $score:expr ),* ) => {
            $(
                pub const $name: Behaviour = ($score,"$name");
            )*
    };
}

// Define behaviours and scores
define_behaviour! {
    CONNECT => 10,
    PING => 10,
    FAILED_TO_PING => -20,
    TIMEOUT => -20,
    SYNC_USELESS => -50,
    UNEXPECTED_MESSAGE => -50,
    UNEXPECTED_DISCONNECT => -10
}
