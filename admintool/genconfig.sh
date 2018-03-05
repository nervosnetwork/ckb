#!/bin/bash
SIZE=4

ROOT_PATH=`pwd`
DATA_PATH=`pwd`/example

rm -rf $DATA_PATH

if [ ! -f "$DATA_PATH" ]; then
    mkdir -p $DATA_PATH
fi

cp $ROOT_PATH/bls.keypairs $DATA_PATH/

for ((ID=0;ID<$SIZE;ID++))
do
    mkdir -p $DATA_PATH/node$ID
    echo "Start generating private Key for Node" $ID "!"
    python create_keys_addr.py $DATA_PATH $ID
    echo "[PrivateKey Path] : " $DATA_PATH/node$ID
    echo "End generating private Key for Node" $ID "!"
done

for ((ID=0;ID<$SIZE;ID++))
do
    echo "Start creating Network Node" $ID "Configuration!"
    python create_config.py $DATA_PATH $ID
    echo "End creating Network Node" $ID "Configuration!"
    echo "########################################################"
done

echo "********************************************************"
echo "WARN: remember then delete all privkey files!!!"