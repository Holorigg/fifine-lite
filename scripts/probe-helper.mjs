// Diagnostic, read-only state query through Genie's already running helper.
// It does not change settings. Run only while the helper is available.
const socket = new WebSocket('ws://127.0.0.1:24925');
const device = { PID: 424, VID: 12610, usage: 0x55aa, usagePage: 0xff00, reportID: 0 };
const send = (event, data) => socket.send(JSON.stringify({
    event,
    payload: data ? { device, data } : { device },
}));
const timeout = setTimeout(() => { console.error('Timed out waiting for both EQ channels'); socket.close(); }, 5000);
const seen = new Set();

socket.addEventListener('open', () => {
    send('openHidDevice');
    setTimeout(() => {
        send('writeDataToHidDevice', [0xa5, 0x5a, 0xfe, 0x00, 0x16]);
        send('getInputReportFromHidDevice');
    }, 250);
    const poll = setInterval(() => {
        if (socket.readyState !== WebSocket.OPEN || seen.size === 2) {
            clearInterval(poll);
            return;
        }
        send('writeDataToHidDevice', [0xa5, 0x5a, 0xfe, 0x00, 0x16]);
        send('getInputReportFromHidDevice');
    }, 700);
});
socket.addEventListener('message', event => {
    const message = JSON.parse(event.data);
    if (message.event === 'receivedFromGetInputReport') {
        const data = message.payload?.data ?? message.data ?? [];
        console.log('length:', data.length, 'head:', data.slice(0, 25));
        if (data.length > 118) {
            const p = parseInt(data[15], 16);
            const channel = parseInt(data[16 + 3 * p], 16);
            console.log('led_count:', p, 'eq_channel:', channel, 'light_switch:', data[117 + 3 * p]);
            seen.add(channel);
        }
        if (seen.has(0) && seen.has(1)) {
            clearTimeout(timeout);
            socket.close();
        }
    }
});
socket.addEventListener('error', event => console.error('WebSocket error:', event.message ?? event.type));
