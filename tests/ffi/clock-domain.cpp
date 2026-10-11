#include <chrono>
#include <time.h>
#include <cassert>
#include <cstdio>
static long long mono(){timespec t{};assert(clock_gettime(CLOCK_MONOTONIC,&t)==0);return t.tv_sec*1000000000LL+t.tv_nsec;}
int main(){for(int i=0;i<10000;i++){auto a=mono();auto b=std::chrono::duration_cast<std::chrono::nanoseconds>(std::chrono::steady_clock::now().time_since_epoch()).count();auto c=mono();assert(a<=b&&b<=c);}puts("Arch native libstdc++ steady_clock matches CLOCK_MONOTONIC: 10000 bracket checks passed");}
