int main(int argc, char* argv[]) {
  if (argc != 3) {
    return 1;
  }
  if (argv[0][0] != 'a') {
    return 2;
  }
  if (argv[1][0] != 'b') {
    return 3;
  }
  if (argv[2][0] != 'c') {
    return 4;
  }
  return 0;

    // int sum = 0;
    // for (int i = 0; i < argc; i++) {
    //   sum += strlen(argv[i]);
    // }
    // return sum;
}
