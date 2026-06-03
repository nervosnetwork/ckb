import assert from "node:assert/strict";
import * as ccc from "@ckb-ccc/core";

const RPC_URL = process.env.CKB_RPC_URL ?? "http://127.0.0.1:8114";
const ACCOUNT_PRIVATE_KEY =
  process.env.OFFCKB_ACCOUNT_PRIVATE_KEY ??
  "0x6109170b275a09ad54877b82f7d9930f88cab5717d484fb4741ae9d1dd078cd6";

const DAO_OUTPUT_INDEX = 2n;
const DAO_CAPACITY = 200n * 100_000_000n;
const FEE_RATE = 1000n;
const MAX_GENERATED_BLOCKS = 300n;

let rpcId = 0;

class DevnetSigner extends ccc.SignerCkbPrivateKey {
  async getAddressObjs() {
    return [await this.getAddressObjSecp256k1()];
  }

  async getRelatedScripts(txLike) {
    const tx = ccc.Transaction.from(txLike);
    const secp256k1 = await this.getAddressObjSecp256k1();
    const scripts = [];

    for (const input of tx.inputs) {
      const {
        cellOutput: { lock },
      } = await input.getCell(this.client);

      if (
        lock.eq(secp256k1.script) &&
        !scripts.some(({ script }) => script.eq(lock))
      ) {
        scripts.push({
          script: lock,
          cellDeps: (
            await this.client.getKnownScript(ccc.KnownScript.Secp256k1Blake160)
          ).cellDeps,
        });
      }
    }

    return scripts;
  }
}

async function rpc(method, params = []) {
  const response = await fetch(RPC_URL, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      id: ++rpcId,
      jsonrpc: "2.0",
      method,
      params,
    }),
  });

  const payload = await response.json();
  if (payload.error) {
    throw new Error(
      `${method} failed: ${payload.error.message ?? JSON.stringify(payload.error)}`,
    );
  }
  return payload.result;
}

async function buildDevnetClient() {
  const [consensus, genesis] = await Promise.all([
    rpc("get_consensus"),
    rpc("get_block_by_number", ["0x0"]),
  ]);

  const cellbaseTxHash = genesis.transactions[0].hash;
  const depGroupTxHash = genesis.transactions[1].hash;
  const secpCodeHash =
    consensus.secp256k1_blake160_sighash_all_type_hash ??
    "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8";

  const scripts = {
    [ccc.KnownScript.NervosDao]: {
      codeHash: consensus.dao_type_hash,
      hashType: "type",
      cellDeps: [
        {
          cellDep: {
            outPoint: {
              txHash: cellbaseTxHash,
              index: DAO_OUTPUT_INDEX,
            },
            depType: "code",
          },
        },
      ],
    },
    [ccc.KnownScript.Secp256k1Blake160]: {
      codeHash: secpCodeHash,
      hashType: "type",
      cellDeps: [
        {
          cellDep: {
            outPoint: {
              txHash: depGroupTxHash,
              index: 0n,
            },
            depType: "depGroup",
          },
        },
      ],
    },
  };

  return new ccc.ClientPublicTestnet({
    url: RPC_URL,
    fallbacks: [RPC_URL],
    scripts,
  });
}

async function generateBlock() {
  await rpc("generate_block");
}

async function generateBlocks(count) {
  for (let i = 0n; i < count; i += 1n) {
    await generateBlock();
  }
}

async function commitTransaction(client, txHash) {
  for (let i = 0; i < 20; i += 1) {
    await generateBlock();
    const response = await client.getTransaction(txHash);
    if (response?.status === "committed") {
      return response;
    }
  }
  throw new Error(`transaction ${txHash} was not committed`);
}

async function generateUntilClaimable(client, claimEpoch) {
  for (let i = 0n; i < MAX_GENERATED_BLOCKS; i += 1n) {
    const tip = await client.getTipHeader();
    if (compareEpoch(tip.epoch, claimEpoch) >= 0) {
      return tip;
    }
    await generateBlock();
  }

  const tip = await client.getTipHeader();
  throw new Error(
    `tip epoch ${formatEpoch(tip.epoch)} did not reach DAO claim epoch ${formatEpoch(
      claimEpoch,
    )}`,
  );
}

function compareEpoch(left, right) {
  for (let i = 0; i < 3; i += 1) {
    if (left[i] > right[i]) {
      return 1;
    }
    if (left[i] < right[i]) {
      return -1;
    }
  }
  return 0;
}

function formatEpoch(epoch) {
  return `${epoch[0].toString()}:${epoch[1].toString()}/${epoch[2].toString()}`;
}

async function addDaoCellDep(client, tx) {
  await tx.addCellDepsOfKnownScripts(client, ccc.KnownScript.NervosDao);
}

async function main() {
  const client = await buildDevnetClient();
  const signer = new DevnetSigner(client, ACCOUNT_PRIVATE_KEY);
  const { script: lock } = await signer.getRecommendedAddressObj();
  const daoType = await ccc.Script.fromKnownScript(
    client,
    ccc.KnownScript.NervosDao,
    "0x",
  );

  await generateBlocks(2n);

  const depositTx = ccc.Transaction.default();
  depositTx.addOutput(
    {
      capacity: DAO_CAPACITY,
      lock,
      type: daoType,
    },
    "0x",
  );
  await addDaoCellDep(client, depositTx);
  await depositTx.completeFeeBy(signer, FEE_RATE);
  const depositHash = await signer.sendTransaction(depositTx);
  await commitTransaction(client, depositHash);

  const depositOutPoint = { txHash: depositHash, index: 0n };
  const depositCell = await client.getCell(depositOutPoint);
  assert(depositCell, "DAO deposit cell should exist");
  assert.equal(
    await depositCell.isNervosDao(client, "deposited"),
    true,
    "deposit output should be a deposited DAO cell",
  );

  const { header: depositHeader } = await client.getCellWithHeader(
    depositOutPoint,
  );
  assert(depositHeader, "deposit header should be available");

  const prepareTx = ccc.Transaction.default();
  prepareTx.addInput({
    previousOutput: depositOutPoint,
    cellOutput: depositCell.cellOutput,
    outputData: depositCell.outputData,
  });
  prepareTx.addOutput(
    {
      capacity: depositCell.cellOutput.capacity,
      lock,
      type: daoType,
    },
    ccc.hexFrom(ccc.numToBytes(depositHeader.number, 8)),
  );
  prepareTx.headerDeps.push(depositHeader.hash);
  await addDaoCellDep(client, prepareTx);
  await prepareTx.completeFeeBy(signer, FEE_RATE);
  const prepareHash = await signer.sendTransaction(prepareTx);
  await commitTransaction(client, prepareHash);

  const prepareOutPoint = { txHash: prepareHash, index: 0n };
  const prepareCell = await client.getCell(prepareOutPoint);
  assert(prepareCell, "DAO prepare cell should exist");
  assert.equal(
    await prepareCell.isNervosDao(client, "withdrew"),
    true,
    "prepare output should be a withdrew DAO cell",
  );

  const { depositHeader: preparedDepositHeader, withdrawHeader } =
    await prepareCell.getNervosDaoInfo(client);
  assert(preparedDepositHeader, "prepared DAO cell should reference deposit header");
  assert(withdrawHeader, "prepared DAO cell should have withdraw header");

  const claimEpoch = ccc.calcDaoClaimEpoch(
    preparedDepositHeader,
    withdrawHeader,
  );
  await generateUntilClaimable(client, claimEpoch);

  const profit = await prepareCell.getDaoProfit(client);
  assert(profit >= 0n, "DAO profit should not be negative");

  const withdrawTx = ccc.Transaction.default();
  withdrawTx.addInput({
    previousOutput: prepareOutPoint,
    since: ccc.Since.from({
      relative: "absolute",
      metric: "epoch",
      value: ccc.epochToHex(claimEpoch),
    }).toNum(),
    cellOutput: prepareCell.cellOutput,
    outputData: prepareCell.outputData,
  });
  withdrawTx.addOutput(
    {
      capacity: prepareCell.cellOutput.capacity + profit,
      lock,
    },
    "0x",
  );
  withdrawTx.headerDeps.push(preparedDepositHeader.hash, withdrawHeader.hash);
  await addDaoCellDep(client, withdrawTx);
  await withdrawTx.completeFeeBy(signer, FEE_RATE);
  const withdrawHash = await signer.sendTransaction(withdrawTx);
  await commitTransaction(client, withdrawHash);

  const withdrawCell = await client.getCell({ txHash: withdrawHash, index: 0n });
  assert(withdrawCell, "withdraw output should exist");
  assert.equal(
    await withdrawCell.isNervosDao(client),
    false,
    "withdraw output should leave DAO",
  );

  console.log(
    JSON.stringify(
      {
        depositHash,
        prepareHash,
        withdrawHash,
        claimEpoch: formatEpoch(claimEpoch),
        profit: profit.toString(),
      },
      null,
      2,
    ),
  );
}

await main();
