CREATE TABLE block(
    id INTEGER PRIMARY KEY,
    block_hash BLOB NOT NULL,
    block_number BIGINT NOT NULL,
    compact_target INT,
    parent_hash BLOB,
    nonce BLOB,
    timestamp BIGINT,
    version INT,
    transactions_root BLOB,
    epoch_number INT,
    epoch_index SMALLINT,
    epoch_length SMALLINT,
    dao BLOB,
    proposals_hash BLOB,
    extra_hash BLOB
);

CREATE TABLE block_association_proposal(
    id INTEGER PRIMARY KEY,
    block_hash BLOB NOT NULL,
    proposal BLOB NOT NULL
);

CREATE TABLE block_association_uncle(
    id INTEGER PRIMARY KEY,
    block_hash BLOB NOT NULL,
    uncle_hash BLOB NOT NULL
);

CREATE TABLE ckb_transaction(
    id INTEGER PRIMARY KEY,
    tx_hash BLOB NOT NULL,
    version INT NOT NULL,
    input_count SMALLINT NOT NULL,
    output_count SMALLINT NOT NULL,
    witnesses BLOB,
    block_hash BLOB NOT NULL,
    tx_index INT NOT NULL
);

CREATE TABLE tx_association_header_dep(
    id INTEGER PRIMARY KEY,
    tx_hash BLOB NOT NULL,
    block_hash BLOB NOT NULL
);

CREATE TABLE tx_association_cell_dep(
    id INTEGER PRIMARY KEY,
    tx_hash BLOB NOT NULL,
    out_point BLOB NOT NULL,
    dep_type SMALLINT NOT NULL
);

CREATE TABLE output(
    id INTEGER PRIMARY KEY,
    out_point BLOB NOT NULL,
    capacity BIGINT NOT NULL,
    data BLOB,
    lock_script_hash BLOB,
    type_script_hash BLOB,
    tx_hash BLOB NOT NULL,
    output_index INT NOT NULL   
);

CREATE TABLE input(
    id INTEGER PRIMARY KEY,
    out_point BLOB NOT NULL,
    since BLOB NOT NULL,
    tx_hash BLOB NOT NULL,
    input_index INT NOT NULL
);

CREATE TABLE script(
    id INTEGER PRIMARY KEY,
    script_hash BLOB NOT NULL,
    code_hash BLOB,
    args BLOB,
    hash_type SMALLINT
);

CREATE INDEX "index_tx_table_tx_hash" ON "ckb_transaction" ("tx_hash");

CREATE INDEX "index_input_table_out_point" ON "input" ("out_point");

CREATE INDEX "index_output_table_out_point" ON "output" ("out_point");
CREATE INDEX "index_output_table_lock" ON "output" ("lock_script_hash");
CREATE INDEX "index_output_table_type" ON "output" ("type_script_hash");
CREATE INDEX "index_output_table_tx_hash" ON "output" ("tx_hash");

CREATE INDEX "index_script_table_script_hash" ON "script" ("script_hash");
CREATE INDEX "index_script_table_script_code_hash" ON "script" ("code_hash");
CREATE INDEX "index_script_table_script_args" ON "script" ("args");
