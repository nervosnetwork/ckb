CREATE TABLE block(
    id SERIAL PRIMARY KEY,
    block_hash BYTEA UNIQUE NOT NULL,
    block_number BIGINT NOT NULL,
    compact_target INTEGER NOT NULL,
    parent_hash BYTEA NOT NULL,
    nonce BYTEA NOT NULL,
    timestamp BIGINT NOT NULL,
    version INTEGER NOT NULL,
    transactions_root BYTEA NOT NULL,
    epoch_number INTEGER NOT NULL,
    epoch_index SMALLINT NOT NULL,
    epoch_length SMALLINT NOT NULL,
    dao BYTEA NOT NULL,
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
    tx_hash BYTEA NOT NULL,
    output_index INTEGER NOT NULL
);

CREATE TABLE input(
    out_point BYTEA PRIMARY KEY,
    since BYTEA NOT NULL,
    tx_hash BYTEA NOT NULL,
    input_index INTEGER NOT NULL
);

CREATE TABLE script(
    id SERIAL PRIMARY KEY,
    script_hash BYTEA UNIQUE NOT NULL,
    script_code_hash BYTEA,
    script_args BYTEA,
    script_type SMALLINT
);

CREATE TABLE output_association_script(
    id SERIAL PRIMARY KEY,
    out_point BYTEA NOT NULL,
    script_hash BYTEA NOT NULL
);