pub use log::{self as internal, Level, SetLoggerError};
pub use serde_json::json;
use serde_json::{Map, Value};

#[doc(hidden)]
#[macro_export]
macro_rules! env {
    ($($inner:tt)*) => {
        env!($($inner)*)
    }
}

#[macro_export(local_inner_macros)]
macro_rules! trace {
    ($( $args:tt )*) => {
        $crate::internal::trace!($( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! debug {
    ($( $args:tt )*) => {
        $crate::internal::debug!($( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! info {
    ($( $args:tt )*) => {
        $crate::internal::info!($( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! warn {
    ($( $args:tt )*) => {
        $crate::internal::warn!($( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! error {
    ($( $args:tt )*) => {
        $crate::internal::error!($( $args )*);
    }
}

/// Enable metrics collection via configuring log filter. Here are some examples:
///   * `filter=info,ckb-metrics=trace`: enable all metrics collection
///   * `filter=info,ckb-metrics=trace,ckb-metrics-sync=off`: enable all metrics collection except
///     topic "sync"
///   * `filter=info,ckb-metrics-sync=trace`: only enable metrics collection of topic "sync"
#[macro_export(local_inner_macros)]
macro_rules! metric {
    ( { $( $args:tt )* } ) => {
        let topic = $crate::__metric_topic!( $($args)* );
        let filter = ::std::format!("ckb-metrics-{}", topic);
        if $crate::log_enabled_target!(&filter, $crate::Level::Trace) {
            let mut metric = $crate::json!( { $( $args )* });
            $crate::__log_metric(&mut metric, $crate::env!("CARGO_PKG_NAME"));
        }
    }
}

#[doc(hidden)]
#[macro_export(local_inner_macros)]
macro_rules! __metric_topic {
     ("topic" : $topic:expr , $($_args:tt)*) => {
         $topic
     };
     ($_key:literal : $_val:tt , $($args:tt)+) => {
        $crate::__metric_topic!($($args)+)
     }
 }

#[macro_export(local_inner_macros)]
macro_rules! log_enabled {
    ($level:expr) => {
        $crate::internal::log_enabled!($level);
    };
}

#[macro_export(local_inner_macros)]
macro_rules! trace_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::trace!(target: $target, $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! debug_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::debug!(target: $target, $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! info_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::info!(target: $target, $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! warn_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::warn!(target: $target, $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! error_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::error!(target: $target, $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! log_enabled_target {
    ($target:expr, $level:expr) => {
        $crate::internal::log_enabled!(target: $target, $level);
    };
}

// Used inside macro `metric!`
pub fn __log_metric(metric: &mut Value, default_target: &str) {
    if metric.get("fields").is_none() {
        metric
            .as_object_mut()
            .map(|obj| obj.insert("fields".to_string(), Map::new().into()));
    }
    if metric.get("tags").is_none() {
        metric
            .as_object_mut()
            .map(|obj| obj.insert("tags".to_string(), Map::new().into()));
    }
    metric.get_mut("tags").and_then(|tags| {
        tags.as_object_mut().map(|tags| {
            if !tags.contains_key("target") {
                tags.insert("target".to_string(), default_target.into());
            }
        })
    });
    trace_target!("ckb-metrics", "{}", metric);
}
