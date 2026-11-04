// SPDX-License-Identifier: Apache-2.0

#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <IOKit/ps/IOPowerSources.h>
#import <IOKit/ps/IOPSKeys.h>
#include <stdint.h>

int32_t wenlan_macos_thermal_state(void) {
    return (int32_t)NSProcessInfo.processInfo.thermalState;
}

double wenlan_macos_seconds_since_last_input(void) {
    return CGEventSourceSecondsSinceLastEventType(
        kCGEventSourceStateCombinedSessionState,
        kCGAnyInputEventType
    );
}

// 1 when the Mac is running on battery, 0 on AC power (or a UPS), -1 when
// the power source cannot be read.
int32_t wenlan_macos_on_battery(void) {
    CFTypeRef info = IOPSCopyPowerSourcesInfo();
    if (info == NULL) {
        return -1;
    }
    CFStringRef type = IOPSGetProvidingPowerSourceType(info);
    int32_t result = -1;
    if (type != NULL) {
        result = CFStringCompare(type, CFSTR(kIOPMBatteryPowerKey), 0) == kCFCompareEqualTo
            ? 1
            : 0;
    }
    CFRelease(info);
    return result;
}
