use ckb_app_config::ExitCode;
use crypto::secp::Generator;
use numext_fixed_hash::H256;

pub fn secp256k1() -> Result<(), ExitCode> {
    let result: H256 = Generator::new().random_privkey().into();
    println!("{:#x}", result);
    Ok(())
}
