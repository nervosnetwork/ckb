/// Helper macro for reducing boilerplate code for build block
///
/// # Examples
///
/// ```
/// use ckb_core::block::{Block, BlockBuilder};
/// use ckb_core::header::HeaderBuilder;
/// use ckb_test_chain_utils::build_block;
///
/// fn block() -> Block{
///     let header = HeaderBuilder::default().build();
///     build_block!(
///         build_unchecked,
///         from_header_builder: {
///             from_header: header.clone(),
///             timestamp: 10,
///         },
///         header: header,
///     )
/// }
///
/// // This is equivalent to:
/// fn expand_block() -> Block {
///     let header = HeaderBuilder::default().build();
///     unsafe {
///         BlockBuilder::from_header_builder(
///             HeaderBuilder::from_header(
///                 header.clone()
///             ).timestamp(10)
///         ).header(
///             header
///         )
///         .build_unchecked()
///     }
/// }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! build_block {
    (build_unchecked, from_header_builder: { $($header_builder:tt)+ }, $($field:ident: $value:expr,)+ ) => {
        unsafe {
            BlockBuilder::from_header_builder(header_builder!($($header_builder)+))
            $(
                .$field($value)
            )+
            .build_unchecked()
        }
    };
    (from_header_builder: { $($header_builder:tt)+ }, $($field:ident: $value:expr,)+ ) => {
        BlockBuilder::from_header_builder(header_builder!($($header_builder)+))
        $(
            .$field($value)
        )+
        .build()
    };
    (build_unchecked, $($field:ident: $value:expr,)+) => {
        unsafe {
            BlockBuilder::default()
            $(
                .$field($value)
            )+
            .build_unchecked()
        }
    };
    ($($field:ident: $value:expr,)+) => {
        BlockBuilder::default()
        $(
            .$field($value)
        )+
        .build()
    };
}

#[macro_export]
macro_rules! header_builder {
    ( from_header: $header:expr, $($field:ident: $value:expr,)+ ) => {
        HeaderBuilder::from_header($header)
        $(
            .$field($value)
        )+
    };
    ( $($field:ident: $value:expr,)+ ) => {
        HeaderBuilder::default()
        $(
            .$field($value)
        )+
    };
}
