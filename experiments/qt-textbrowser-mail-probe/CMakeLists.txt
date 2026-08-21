cmake_minimum_required(VERSION 3.21)
project(qt_textbrowser_mail_probe LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

find_package(Qt6 REQUIRED COMPONENTS Widgets)

add_executable(qt-textbrowser-mail-probe main.cpp)
target_link_libraries(qt-textbrowser-mail-probe PRIVATE Qt6::Widgets)

