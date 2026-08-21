cmake_minimum_required(VERSION 3.24)
project(kio-thumbnail-probe LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

find_package(Qt6 REQUIRED COMPONENTS Core Gui Widgets)
find_package(KF6KIO REQUIRED COMPONENTS Gui)

add_executable(kio-thumbnail-probe main.cpp)
target_link_libraries(kio-thumbnail-probe PRIVATE Qt6::Core Qt6::Gui Qt6::Widgets KF6::KIOGui)
