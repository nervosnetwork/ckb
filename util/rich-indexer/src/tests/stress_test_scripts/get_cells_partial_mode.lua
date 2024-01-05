wrk.method = "POST"
wrk.headers["Content-Type"] = "application/json"

wrk.body = [[
{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_cells",
    "params": [
        {
            "script": {
                "code_hash": "0xbbad126377d45f90a8ee120da988a2d7332c78ba8fd679aab478a19d6c133494",
                "hash_type": "data1",
                "args": "0x"
            },
            "script_type": "type",
            "script_search_mode": "partial",
            "filter": {
                "output_data": "0x0000",
                "output_data_filter_mode": "partial"
            },
            "with_data": false
        },
        "asc",
        "0x64"
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
-- wrk -t4 -c100 -d60s -s ./util/rich-indexer/src/tests/stress_test_scripts/get_cells_partial_mode.lua --latency http://127.0.0.1:8114