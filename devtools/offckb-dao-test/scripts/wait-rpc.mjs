const url = process.argv[2] ?? "http://127.0.0.1:8114";
const deadline = Date.now() + 60_000;

async function isReady() {
  try {
    const response = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        id: 1,
        jsonrpc: "2.0",
        method: "get_tip_block_number",
        params: [],
      }),
    });
    const payload = await response.json();
    return Boolean(payload.result);
  } catch {
    return false;
  }
}

while (Date.now() < deadline) {
  if (await isReady()) {
    process.exit(0);
  }
  await new Promise((resolve) => setTimeout(resolve, 1000));
}

console.error(`CKB RPC did not become ready at ${url}`);
process.exit(1);
