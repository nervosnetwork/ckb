use crate::{H160, H256, H512, H520};

macro_rules! impl_std_fmt {
    ($name:ident, $bytes_size:expr) => {
        impl ::std::fmt::Debug for $name {
            #[inline]
            fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                write!(f, stringify!($name))?;
                write!(f, " ( [")?;
                write!(f, " {:#04x}", self.0[0])?;
                for chr in self.0[1..].iter() {
                    write!(f, ", {:#04x}", chr)?;
                }
                write!(f, " ] )")
            }
        }
        impl ::std::fmt::LowerHex for $name {
            #[inline]
            fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                let alternate = f.alternate();
                if alternate {
                    write!(f, "0x")?;
                }
                for x in self.0.iter() {
                    write!(f, "{:02x}", x)?;
                }
                Ok(())
            }
        }
        impl ::std::fmt::Display for $name {
            #[inline]
            fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                let alternate = f.alternate();
                if alternate {
                    write!(f, "0x")?;
                }
                for x in self.0.iter() {
                    write!(f, "{:02x}", x)?;
                }
                Ok(())
            }
        }
    };
}

impl_std_fmt!(H160, 20);
impl_std_fmt!(H256, 32);
impl_std_fmt!(H512, 64);
impl_std_fmt!(H520, 65);
