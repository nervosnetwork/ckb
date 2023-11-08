CREATE TABLE block(
    id INTEGER PRIMARY KEY,
    block_hash BLOB UNIQUE NOT NULL,
    block_number BIGINT NOT NULL,
    compact_target INT NOT NULL,
    parent_hash BLOB NOT NULL,
    nonce BLOB NOT NULL,
    timestamp BIGINT NOT NULL,
    version INT NOT NULL,
    transactions_root BLOB NOT NULL,
    epoch_number INT NOT NULL,
    epoch_index SMALLINT NOT NULL,
    epoch_length SMALLINT NOT NULL,
    dao BLOB NOT NULL,
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
    tx_hash BLOB UNIQUE NOT NULL,
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
    out_point BLOB UNIQUE NOT NULL,
    capacity BIGINT NOT NULL,
    data BLOB,
    tx_hash BLOB NOT NULL,
    output_index INT NOT NULL
);

CREATE TABLE input(
    out_point BLOB PRIMARY KEY,
    since BIGINT NOT NULL,
    tx_hash BLOB NOT NULL,
    input_index INT NOT NULL
);

CREATE TABLE script(
    id INTEGER PRIMARY KEY,
    script_hash BLOB UNIQUE NOT NULL,
    script_code_hash BLOB,
    script_args BLOB,
    script_type SMALLINT
);

CREATE TABLE output_association_script(
    id INTEGER PRIMARY KEY,
    out_point BLOB NOT NULL,
    script_hash BLOB NOT NULL
);