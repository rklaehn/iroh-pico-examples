/*
 * BLE scanning on Presto's RM2/CYW43439 through the Pico SDK's BTstack port.
 *
 * BTstack runs on the CYW43 async context task. Advertising reports are
 * forwarded to a single callback on that task, with the async context lock
 * held, so the callback must be short and must not call back into BTstack.
 */
#include <stdio.h>
#include <stdint.h>
#include "pico/cyw43_arch.h"
#include "btstack.h"

typedef void (*presto_bt_report_fn)(const uint8_t address[6],
                                    uint8_t address_type, int8_t rssi,
                                    const uint8_t *data, uint8_t data_len);

static presto_bt_report_fn report_fn;
static btstack_packet_callback_registration_t hci_event_registration;

static void packet_handler(uint8_t packet_type, uint16_t channel,
                           uint8_t *packet, uint16_t size)
{
    (void)channel;
    (void)size;
    if (packet_type != HCI_EVENT_PACKET) return;

    switch (hci_event_packet_get_type(packet)) {
    case BTSTACK_EVENT_STATE:
        if (btstack_event_state_get_state(packet) != HCI_STATE_WORKING) break;
        printf("Bluetooth is up; scanning\n");
        // Active scan so scan responses (which often carry the name) arrive
        // too. Interval == window (30 ms) keeps the radio listening all the time.
        gap_set_scan_parameters(1, 48, 48);
        gap_start_scan();
        break;
    case GAP_EVENT_ADVERTISING_REPORT: {
        bd_addr_t address;
        gap_event_advertising_report_get_address(packet, address);
        report_fn(address,
                  gap_event_advertising_report_get_address_type(packet),
                  (int8_t)gap_event_advertising_report_get_rssi(packet),
                  gap_event_advertising_report_get_data(packet),
                  gap_event_advertising_report_get_data_length(packet));
        break;
    }
    default:
        break;
    }
}

int presto_bt_start_scan(presto_bt_report_fn on_report)
{
    printf("Initializing RM2/CYW43439 Bluetooth...\n");
    stdio_flush();
    int result = cyw43_arch_init();
    if (result != 0) {
        printf("CYW43 initialization failed: %d\n", result);
        stdio_flush();
        return result;
    }

    report_fn = on_report;
    async_context_t *context = cyw43_arch_async_context();
    async_context_acquire_lock_blocking(context);
    hci_event_registration.callback = &packet_handler;
    hci_add_event_handler(&hci_event_registration);
    result = hci_power_control(HCI_POWER_ON);
    async_context_release_lock(context);
    return result;
}
