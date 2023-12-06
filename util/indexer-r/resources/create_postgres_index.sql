CREATE INDEX CONCURRENTLY "index_block_table_block_hash" ON "block" ("block_hash");

CREATE INDEX CONCURRENTLY "index_input_table_out_point" ON "input" ("out_point");
CREATE INDEX CONCURRENTLY "index_input_table_tx_hash" ON "input" ("tx_hash");

CREATE INDEX CONCURRENTLY "index_tx_table_tx_hash" ON "ckb_transaction" ("tx_hash");
CREATE INDEX CONCURRENTLY "index_tx_table_block_hash" ON "ckb_transaction" ("block_hash");

CREATE INDEX CONCURRENTLY "index_output_table_out_point" ON "output" ("out_point");
CREATE INDEX CONCURRENTLY "index_output_table_tx_hash" ON "output" ("tx_hash");
CREATE INDEX CONCURRENTLY "index_output_table_lock" ON "output" ("lock_script_hash");
CREATE INDEX CONCURRENTLY "index_output_table_type" ON "output" ("type_script_hash");
CREATE INDEX CONCURRENTLY "index_script_table_script_code_hash" ON "script" ("code_hash", "hash_type", "args");