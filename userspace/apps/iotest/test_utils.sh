#!/bin/oksh

echo "Testing grep..."
echo "hello world" > grep_test.txt
grep "hello" grep_test.txt
grep -n "world" grep_test.txt
rm grep_test.txt

echo "Testing wc..."
echo "one two three" > wc_test.txt
wc -w wc_test.txt
wc -c wc_test.txt
rm wc_test.txt

echo "Testing cp..."
echo "original content" > cp_src.txt
cp cp_src.txt cp_dest.txt
cat cp_dest.txt
rm cp_src.txt
rm cp_dest.txt

echo "All tests finished."
