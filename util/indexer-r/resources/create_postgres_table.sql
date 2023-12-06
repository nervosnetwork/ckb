CREATE TABLE block(
    id SERIAL PRIMARY KEY,
    block_hash BYTEA UNIQUE NOT NULL,
    block_number BIGINT NOT NULL,
    compact_target INTEGER,
    parent_hash BYTEA,
    nonce BYTEA,
    timestamp BIGINT,
    version INTEGER,
    transactions_root BYTEA,
    epoch_number INTEGER,
    epoch_index SMALLINT,
    epoch_length SMALLINT,
    dao BYTEA,
    proposals_hash BYTEA,
    extra_hash BYTEA
);

CREATE TABLE block_association_proposal(
    id SERIAL PRIMARY KEY,
    block_hash BYTEA NOT NULL,
    proposal BYTEA NOT NULL
);

CREATE TABLE block_association_uncle(
    id SERIAL PRIMARY KEY,
    block_hash BYTEA NOT NULL,
    uncle_hash BYTEA NOT NULL
);

CREATE TABLE ckb_transaction(
    id SERIAL PRIMARY KEY,
    tx_hash BYTEA UNIQUE NOT NULL,
    version INTEGER NOT NULL,
    input_count SMALLINT NOT NULL,
    output_count SMALLINT NOT NULL,
    witnesses BYTEA,
    block_hash BYTEA NOT NULL,
    tx_index INTEGER NOT NULL
);

CREATE TABLE tx_association_header_dep(
    id SERIAL PRIMARY KEY,
    tx_hash BYTEA NOT NULL,
    block_hash BYTEA NOT NULL
);

CREATE TABLE tx_association_cell_dep(
    id SERIAL PRIMARY KEY,
    tx_hash BYTEA NOT NULL,
    out_point BYTEA NOT NULL,
    dep_type SMALLINT NOT NULL
);

CREATE TABLE output(
    id SERIAL PRIMARY KEY,
    out_point BYTEA UNIQUE NOT NULL,
    capacity BIGINT NOT NULL,
    data BYTEA,
    lock_script_hash BYTEA,
    type_script_hash BYTEA,
    tx_hash BYTEA NOT NULL,
    output_index INT NOT NULL   
);

CREATE TABLE input(
    id SERIAL PRIMARY KEY,
    out_point BYTEA UNIQUE NOT NULL,
    since BYTEA NOT NULL,
    tx_hash BYTEA NOT NULL,
    input_index INTEGER NOT NULL
);

CREATE TABLE script(
    id SERIAL PRIMARY KEY,
    script_hash BYTEA UNIQUE NOT NULL,
    code_hash BYTEA,
    args BYTEA,
    hash_type SMALLINT
);

CREATE INDEX "index_output_table_lock" ON "output" ("lock_script_hash");
CREATE INDEX "index_output_table_type" ON "output" ("type_script_hash");
CREATE INDEX "index_output_table_tx_hash" ON "output" ("tx_hash");
CREATE INDEX "index_script_table_script_code_hash" ON "script" ("code_hash", "hash_type", "args");