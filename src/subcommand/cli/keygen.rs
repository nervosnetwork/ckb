use ckb_app_config::ExitCode;
use crypto::secp::Generator;
use numext_fixed_hash::H256;

pub fn keygen() -> Result<(), ExitCode> {
    let result: H256 = Generator::new().random_privkey().into();
    println!("{:#x}", result);
    Ok(())
}
