#!/usr/bin/env ruby
require 'json'
require 'net/http'
require 'uri'

URL      = ARGV[0] || "http://localhost:8114"
CKB_BIN  = ARGV[1] || "./target/debug/ckb"
ACCOUNTS = [
    {
        name: "miner",
        type_hash: "321c1ca2887fb8eddaaa7e917399f71e63e03a1c83ff75ed12099a01115ea2ff",
        private_key: "e79f3207ea4980b7fed79956d5934249ceac4751a4fae01a0f7c4a96884bc4e3",
        utxo: []
    },
    {
        name: "alice",
        type_hash: "67295822d1f852602a7d398edfe9ad42734cf41999c084ea8a401ff7cc994cfa",
        private_key: "76e853efa8245389e33f6fe49dcbd359eb56be2f6c3594e12521d2a806d32156",
        utxo: []
    },
    {
        name: "bob",
        type_hash: "45996d79e9403f99eb2d79c4f649d45be4b5ab8458e359f5421ff1ab4e88e9f4",
        private_key: "9f7fd78dffeda83b77c5c2d7eeaccb05120457787defdbb46da6d2186bf28f13",
        utxo: []
    }
]

class Fixnum
    def random_split(set = nil, repeats = false)
        set ||= 1..self
        set = [*set]
        return if set.empty? || set.min > self || set.inject(0, :+) < self
        tried_numbers = []
        while (not_tried = (set - tried_numbers).select {|n| n <= self }).any?
            tried_numbers << number = not_tried.sample
            return [number] if number == self
            new_set = set.dup
            new_set.delete_at(new_set.index(number)) unless repeats
            randomized_rest = (self-number).random_split(new_set, repeats)
            return [number] + randomized_rest if randomized_rest
        end
    end
end

def pull_transactions
    number = rpc('get_tip_header')[:raw][:number]
    block_hash = rpc('get_block_hash', "[#{number}]")
    block = rpc('get_block', "[\"#{block_hash}\"]")
    block[:transactions].each do |tx|
        tx[:transaction][:outputs].each_with_index do |output, i|
            if match = ACCOUNTS.find{|account| output[:lock] == "0x#{account[:type_hash]}" }
                match[:utxo] << {hash: tx[:hash], index: i, capacity: output[:capacity]}
            end
        end
    end
end

def send_transactions
    account = ACCOUNTS.sample
    if account[:utxo].size > 0
        utxo = account[:utxo].sample(account[:utxo].size / 2 + 1)
        total = utxo.inject(0){|s, o| s + o[:capacity]}
        inputs = utxo.map do |o|
            account[:utxo].delete(o)
            {
                previous_output: {
                    hash: o[:hash],
                    index: o[:index]
                },
                unlock: {
                    version: 0,
                    args: [account[:name].bytes.to_a],
                    signed_args: []
                }
            }
        end
        outputs = total.random_split(100..10000).map do |capacity|
            {
                capacity: capacity,
                data: [0],
                lock: "0x#{ACCOUNTS.sample[:type_hash]}"
            }
        end
        transaction = {
            version: 0,
            deps: [],
            inputs: inputs,
            outputs: outputs,
        }
        signed = `#{CKB_BIN} cli sign -p #{account[:private_key]} -u '#{transaction.to_json}'`
        rpc('send_transaction', "[#{signed}]")
    end
end

def rpc(method, params = "null")
    puts "rpc method: #{method}, params: #{params}"
    response = Net::HTTP.post(
        URI(URL),
        "{\"id\": 1, \"jsonrpc\": \"2.0\", \"method\": \"#{method}\", \"params\": #{params}}",
        "Content-Type" => "application/json")
    JSON.parse(response.body, symbolize_names: true)[:result]
end

10.times do
    pull_transactions
    send_transactions
    sleep(5)
end
