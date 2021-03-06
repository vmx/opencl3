// Copyright (c) 2021 Via Technology Ltd. All Rights Reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

extern crate opencl3;

use cl3::device::{CL_DEVICE_SVM_FINE_GRAIN_BUFFER, CL_DEVICE_TYPE_GPU};
use opencl3::context::Context;
use opencl3::device::Device;
use opencl3::kernel::ExecuteKernel;
use opencl3::platform::get_platforms;
use opencl3::svm::SvmVec;
use opencl3::types::cl_int;
use std::ffi::CString;
use std::ptr;

// The OpenCL kernels in PROGRAM_SOURCE below use built-in work-group functions:
// work_group_reduce_add, work_group_scan_inclusive_add and work_group_broadcast
// which were introduced in OpenCL 2.0.
const PROGRAM_SOURCE: &str = r#"
kernel void sum_int (global int* sums,
                    global int const* values)
{
    int value = work_group_reduce_add(values[get_global_id(0)]);

    if (0u == get_local_id(0))
        sums[get_group_id(0)] = value;
}

kernel void inclusive_scan_int (global int* output,
                                global int const* values)
{
    int sum = 0;
    size_t lid = get_local_id(0);
    size_t lsize = get_local_size(0);

    size_t num_groups = get_num_groups(0);
    for (size_t i = 0u; i < num_groups; ++i)
    {
        size_t lidx = i * lsize + lid;
        int value = work_group_scan_inclusive_add(values[lidx]);
        output[lidx] = sum + value;

        sum += work_group_broadcast(value, lsize - 1);
    }
}"#;

const PROGRAM_BUILD_OPTIONS: &str = "-cl-std=CL2.0 ";
const SUM_KERNEL_NAME: &str = "sum_int";
const INCLUSIVE_SCAN_KERNEL_NAME: &str = "inclusive_scan_int";

#[test]
#[ignore]
fn test_opencl_2_kernel_example() {
    let platforms = get_platforms().unwrap();
    assert!(0 < platforms.len());

    /////////////////////////////////////////////////////////////////////
    // Query OpenCL compute environment
    let opencl_2: String = "OpenCL 2".to_string();

    // Find an OpenCL fine grained SVM, platform and device
    let mut device_id = ptr::null_mut();
    let mut is_fine_grained_svm: bool = false;
    for p in platforms {
        let platform_version = p.version().unwrap().into_string().unwrap();
        if platform_version.contains(&opencl_2) {
            let devices = p
                .get_devices(CL_DEVICE_TYPE_GPU)
                .expect("Platform::get_devices failed");

            for dev_id in devices {
                let device = Device::new(dev_id);
                let svm_mem_capability = device.svm_mem_capability();
                is_fine_grained_svm = 0 < svm_mem_capability & CL_DEVICE_SVM_FINE_GRAIN_BUFFER;
                if is_fine_grained_svm {
                    device_id = dev_id;
                    break;
                }
            }
        }
    }

    if is_fine_grained_svm {
        // Create OpenCL context from the OpenCL svm device
        let device = Device::new(device_id);
        let vendor = device.vendor().unwrap();
        let vendor_id = device.vendor_id().unwrap();
        println!("OpenCL device vendor name: {:?}", vendor);
        println!("OpenCL device vendor id: {:X}", vendor_id);

        /////////////////////////////////////////////////////////////////////
        // Initialise OpenCL compute environment

        // Create OpenCL context from the OpenCL svm device
        // and an OpenCL command queue for the device
        let mut context = Context::from_device(device).unwrap();
        context.create_command_queues_with_properties(0, 0).unwrap();

        // Build the OpenCL 2.0 program source and create the kernels.
        let src = CString::new(PROGRAM_SOURCE).unwrap();
        let options = CString::new(PROGRAM_BUILD_OPTIONS).unwrap();
        context.build_program_from_source(&src, &options).unwrap();

        assert!(!context.kernels().is_empty());
        for kernel_name in context.kernels().keys() {
            println!("Kernel name: {:?}", kernel_name);
        }

        // Get the svm capability of all the devices in the context.
        let svm_capability = context.get_svm_mem_capability();
        assert!(0 < svm_capability);

        // Create SVM vectors for the input and output data

        // The input data
        const ARRAY_SIZE: usize = 8;
        let value_array: [cl_int; ARRAY_SIZE] = [3, 2, 5, 9, 7, 1, 4, 2];

        // Copy into an OpenCL SVM vector
        let mut test_values = SvmVec::<cl_int>::with_capacity(&context, svm_capability, ARRAY_SIZE);
        for &val in value_array.iter() {
            test_values.push(val);
        }

        // The output data, an OpenCL SVM vector
        let mut results =
            SvmVec::<cl_int>::with_capacity_zeroed(&context, svm_capability, ARRAY_SIZE);
        unsafe { results.set_len(ARRAY_SIZE) };

        // Get the command queue for the device
        let queue = context.default_queue();

        // Run the sum kernel on the input data
        let sum_kernel_name = CString::new(SUM_KERNEL_NAME).unwrap();
        if let Some(sum_kernel) = context.get_kernel(&sum_kernel_name) {
            let sum_kernel_event = ExecuteKernel::new(sum_kernel)
                .set_arg_svm(results.as_mut_ptr())
                .set_arg_svm(test_values.as_ptr())
                .set_global_work_size(ARRAY_SIZE)
                .enqueue_nd_range(&queue)
                .unwrap();

            // Wait for the kernel to complete execution on the device
            sum_kernel_event.wait().unwrap();

            // Can access OpenCL SVM directly, no need to map or read the results
            println!("sum results: {:?}", results);
            assert_eq!(33, results[0]);
            assert_eq!(0, results[ARRAY_SIZE - 1]);
        }

        // Run the inclusive scan kernel on the input data
        let inclusive_scan_kernel_name = CString::new(INCLUSIVE_SCAN_KERNEL_NAME).unwrap();
        if let Some(inclusive_scan_kernel) = context.get_kernel(&inclusive_scan_kernel_name) {
            let kernel_event = ExecuteKernel::new(inclusive_scan_kernel)
                .set_arg_svm(results.as_mut_ptr())
                .set_arg_svm(test_values.as_ptr())
                .set_global_work_size(ARRAY_SIZE)
                .enqueue_nd_range(&queue)
                .unwrap();

            kernel_event.wait().unwrap();

            println!("inclusive_scan results: {:?}", results);
            assert_eq!(value_array[0], results[0]);
            assert_eq!(33, results[ARRAY_SIZE - 1]);
        }
    } else {
        println!("OpenCL fine grained SVM capable device not found");
    }
}
