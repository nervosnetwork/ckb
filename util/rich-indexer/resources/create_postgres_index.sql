CREATE INDEX "index_block_association_proposal_table_block_id" ON "block_association_proposal" ("block_id");
CREATE INDEX "index_block_association_uncle_table_block_id" ON "block_association_uncle" ("block_id");

CREATE INDEX "index_tx_table_tx_hash" ON "ckb_transaction" ("tx_hash");
CREATE INDEX "index_tx_table_block_id" ON "ckb_transaction" ("block_id");
CREATE INDEX "index_tx_association_header_dep_table_tx_id" ON "tx_association_header_dep" ("tx_id");
CREATE INDEX "index_tx_association_cell_dep_table_tx_id" ON "tx_association_cell_dep" ("tx_id");

CREATE INDEX "idx_output_table_tx_id_output_index" ON "output" ("tx_id", "output_index");
CREATE INDEX "idx_output_table_lock_script_id" ON "output" ("lock_script_id");
CREATE INDEX "idx_output_table_type_script_id" ON "output" ("type_script_id");

CREATE INDEX "idx_input_table_consumed_tx_id" ON "input" ("consumed_tx_id");