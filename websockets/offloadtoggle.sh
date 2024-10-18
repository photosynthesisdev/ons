if [ -z "$1" ]; then
    echo "Usage: $0 <0|1>"
    echo "0: Disable TCP offloading"
    echo "1: Enable TCP offloading"
    exit 1
fi
INTERFACE="eno1"
if [ "$1" -eq 0 ]; then
    STATE="off"
    echo "Disabling TCP offloading on $INTERFACE..."
elif [ "$1" -eq 1 ]; then
    STATE="on"
    echo "Enabling TCP offloading on $INTERFACE..."
else
    echo "Invalid argument. Use 0 to disable, 1 to enable."
    exit 1
fi
sudo ethtool -K $INTERFACE tso $STATE
sudo ethtool -K $INTERFACE gro $STATE
sudo ethtool -K $INTERFACE gso $STATE
echo "TCP offloading has been set to $STATE on $INTERFACE."