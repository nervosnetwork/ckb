CREATE TABLE block(
    id INTEGER PRIMARY KEY,
    block_hash BLOB,
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
    uncles_hash BLOB
);

CREATE TABLE proposal(
    id INTEGER PRIMARY KEY,
    proposal BLOB
);

CREATE TABLE block_association_proposal(
    id INTEGER PRIMARY KEY,
    block_hash_id INTEGER NOT NULL,
    proposal_id INTEGER NOT NULL
);

CREATE TABLE block_association_uncle(
    id INTEGER PRIMARY KEY,
    block_hash_id INTEGER NOT NULL,
    uncle_hash_id INTEGER NOT NULL
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

CREATE TABLE cell_dep(
    output_id INTEGER PRIMARY KEY,
    dep_type SMALLINT NOT NULL
);

CREATE TABLE tx_association_cell_dep(
    id INTEGER PRIMARY KEY,
    tx_id INTEGER NOT NULL,
    cell_dep_id INTEGER NOT NULL
);

CREATE TABLE header_dep(
    block_id INTEGER PRIMARY KEY
);

CREATE TABLE tx_association_header_dep(
    id INTEGER PRIMARY KEY,
    tx_id INTEGER NOT NULL,
    block_id INTEGER NOT NULL
);

CREATE TABLE input(
    output_id INTEGER PRIMARY KEY,
    since BIGINT NOT NULL,
    tx_hash BLOB NOT NULL,
    input_index INT NOT NULL
);

CREATE TABLE output(
    id INTEGER PRIMARY KEY,
    outpoint BLOB NOT NULL,
    capacity BIGINT NOT NULL,
    data BLOB,
    tx_hash BLOB NOT NULL,
    output_index INT NOT NULL
);

CREATE TABLE script(
    id INTEGER PRIMARY KEY,
    script_hash BLOB,
    script_code_hash BLOB,
    script_args BLOB,
    script_type SMALLINT
);

CREATE TABLE output_association_script(
    id INTEGER PRIMARY KEY,
    output_id INTEGER NOT NULL,
    script_id INTEGER NOT NULL
);