wrk.method = "POST"
wrk.headers["Content-Type"] = "application/json"

wrk.body = [[
{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_cells_capacity",
    "params": [
        {
            "script": {
                "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
                "hash_type": "type",
                "args": "0x5989ae415b"
            },
            "script_type": "lock",
            "script_search_mode": "prefix"
        }
    ]
}
]]


function response(status, headers, body)
    if (string.find(body, '"error"')) then
        print('error, resp: ', body)
        wrk.thread:stop()
    end
end

-- This command is run under the condition that the CPU has 4 cores
-- wrk -t4 -c100 -d60s -s ./util/rich-indexer/src/tests/stress_test_scripts/get_cells_capacity_prefix.lua --latency http://127.0.0.1:8114
