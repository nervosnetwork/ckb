use ckb_types::{
    core::BlockView,
    packed::{Bytes, CellOutput},
};
use numext_fixed_uint::U256;
use rhai::{Engine, EvalAltResult, Scope, AST};

/// Custom filters
///
/// base on embedded scripting language Rhai
pub struct CustomFilters {
    engine: Engine,
    block_filter: Option<AST>,
    cell_filter: Option<AST>,
}

impl Clone for CustomFilters {
    fn clone(&self) -> Self {
        CustomFilters {
            engine: Engine::new(),
            block_filter: self.block_filter.clone(),
            cell_filter: self.cell_filter.clone(),
        }
    }
}

fn to_uint(s: &str) -> Result<U256, Box<EvalAltResult>> {
    match &s[..2] {
        "0b" => U256::from_bin_str(&s[2..]),
        "0o" => U256::from_oct_str(&s[2..]),
        "0x" => U256::from_hex_str(&s[2..]),
        _ => U256::from_dec_str(s),
    }
    .map_err(|e| e.to_string().into())
}

macro_rules! register_ops {
    ($engine:ident $(, $op:tt)+ $(,)?) => {
        $(
            $engine.register_fn(stringify!($op), |a: U256, b: U256| a $op b);
        )+
    };
}

impl CustomFilters {
    /// Construct new CustomFilters
    pub fn new(block_filter_str: Option<&str>, cell_filter_str: Option<&str>) -> Self {
        let mut engine = Engine::new();
        engine.register_fn("to_uint", to_uint);
        register_ops!(engine, +, -, *, /, %, ==, !=, <, <=, >, >=);

        let block_filter = block_filter_str.map(|block_filter| {
            engine
                .compile(block_filter)
                .expect("compile block_filter should be ok")
        });
        let cell_filter = cell_filter_str.map(|cell_filter| {
            engine
                .compile(cell_filter)
                .expect("compile cell_filter should be ok")
        });

        Self {
            engine,
            block_filter,
            cell_filter,
        }
    }

    /// Returns true if the block filter is match
    pub fn is_block_filter_match(&self, block: &BlockView) -> bool {
        self.block_filter
            .as_ref()
            .map(|block_filter| {
                let json_block: ckb_jsonrpc_types::BlockView = block.clone().into();
                let parsed_block = self
                    .engine
                    .parse_json(serde_json::to_string(&json_block).unwrap(), true)
                    .unwrap();
                let mut scope = Scope::new();
                scope.push("block", parsed_block);
                self.engine
                    .eval_ast_with_scope(&mut scope, block_filter)
                    .expect("eval block_filter should be ok")
            })
            .unwrap_or(true)
    }

    /// Returns true if the cell filter is match
    pub fn is_cell_filter_match(&self, output: &CellOutput, output_data: &Bytes) -> bool {
        self.cell_filter
            .as_ref()
            .map(|cell_filter| {
                let json_output: ckb_jsonrpc_types::CellOutput = output.clone().into();
                let parsed_output = self
                    .engine
                    .parse_json(serde_json::to_string(&json_output).unwrap(), true)
                    .unwrap();
                let mut scope = Scope::new();
                scope.push("output", parsed_output);
                scope.push("output_data", format!("{output_data:#x}"));
                self.engine
                    .eval_ast_with_scope(&mut scope, cell_filter)
                    .expect("eval cell_filter should be ok")
            })
            .unwrap_or(true)
    }
}
