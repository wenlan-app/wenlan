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

// The charge in percent (0-100) when the Mac is running on battery; -1 on
// AC power, on a UPS, or when the power source cannot be read.
int32_t wenlan_macos_battery_percent_on_battery(void) {
    CFTypeRef info = IOPSCopyPowerSourcesInfo();
    if (info == NULL) {
        return -1;
    }
    int32_t percent = -1;
    CFStringRef type = IOPSGetProvidingPowerSourceType(info);
    if (type != NULL
        && CFStringCompare(type, CFSTR(kIOPMBatteryPowerKey), 0) == kCFCompareEqualTo) {
        CFArrayRef list = IOPSCopyPowerSourcesList(info);
        if (list != NULL) {
            for (CFIndex i = 0; i < CFArrayGetCount(list) && percent < 0; i++) {
                CFDictionaryRef source =
                    IOPSGetPowerSourceDescription(info, CFArrayGetValueAtIndex(list, i));
                if (source == NULL) {
                    continue;
                }
                CFNumberRef current = CFDictionaryGetValue(source, CFSTR(kIOPSCurrentCapacityKey));
                CFNumberRef max = CFDictionaryGetValue(source, CFSTR(kIOPSMaxCapacityKey));
                int current_value = 0;
                int max_value = 0;
                if (current != NULL && max != NULL
                    && CFNumberGetValue(current, kCFNumberIntType, &current_value)
                    && CFNumberGetValue(max, kCFNumberIntType, &max_value)
                    && max_value > 0) {
                    percent = (int32_t)((current_value * 100) / max_value);
                }
            }
            CFRelease(list);
        }
    }
    CFRelease(info);
    return percent;
}
