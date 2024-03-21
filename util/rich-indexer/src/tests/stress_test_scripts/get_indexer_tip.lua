wrk.method = "POST"
wrk.headers["Content-Type"] = "application/json"

wrk.body = [[
{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_indexer_tip"
}
]]


function response(status, headers, body)
    if (string.find(body, '"error"')) then
        print('error, resp: ', body)
        wrk.thread:stop()
    end
end

-- This command is run under the condition that the CPU has 4 cores
-- wrk -t4 -c100 -d60s -s ./util/rich-indexer/src/tests/stress_test_scripts/get_indexer_tip.lua --latency http://127.0.0.1:8114
